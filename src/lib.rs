//! DiscordSRV as an arismoko native mod (trait mod on `mod_api`). The logic is the unchanged
//! `discordsrv-bridge` crate in `core/` (gateway, REST, console channel, chat relay, linking); this
//! file only wires it to hooks, host methods and the config file. Gaps are marked
//! `TODO(arismoko-api: <name>)`; PORT_NOTES.md lists them with what each one costs.
//!
//! Config: `config/discordsrv/config.toml`, written with the defaults on the first start. The
//! defaults are off: no console channel ID, no chat channels, no token file. The bridge also reads
//! the Paper plugin's `plugins/DiscordSRV/config.yml` from the working directory when it exists.
//!
//! # API for other mods (CoreConsole depends on this)
//!
//! Declare `[dependencies] discordsrv = "^0.3"` in the dependent's `horizon-mod.toml` (this mod's
//! version is 0.3.0), then call the functions below through the `discordsrv` alias. Preprocessor
//! API, the same as the bridge's:
//!
//! - `set_console_preprocessor(f: Preprocessor)` with
//!   `type Preprocessor = Box<dyn Fn(&str, &str) -> Preprocess + Send + Sync>`.
//!   `f(author_id, command)` gets the Discord user ID of the console channel message and the command
//!   without its leading slash. It runs on the bridge's gateway thread, after the bot check and
//!   before the console command list, so a command it returns must still pass the list. It must not
//!   call `set_console_preprocessor`. One preprocessor is kept; a later call replaces it.
//! - `enum Preprocess { Run(String), Refuse { reply: String } }`: `Run` runs that command (the
//!   message as it came, or another one), `Refuse` runs nothing and posts `reply` to the channel.
//!
//! Until a preprocessor is set, a console channel message runs as it came.
use discordsrv_bridge as bridge;
use mod_api::{
    Command, ConsoleLineEvent, Config, ConfigKind, Flow, Hooks, HostSide,
    PlayerAdvancementDoneEvent, PlayerCommandEvent, PlayerDiedEvent, PlayerLoggedInEvent,
    PlayerLoggedOutEvent, PlayerLoginEvent, RegisterCommandsEvent, RegisterConfigsEvent,
    ServerAboutToStartEvent, ServerChatEvent, ServerHost, ServerStoppingEvent, ServerTickEvent,
    ServerTickPhase, Text,
};
use std::path::Path;

pub use discordsrv_bridge::{Preprocess, Preprocessor, set_console_preprocessor};

const ID: &str = "discordsrv";
/// Relative to the server's `config/` folder.
const CONFIG_FILE: &str = "discordsrv/config.toml";
const GAME_VERSION: &str = "26.3";

#[derive(Default)]
struct Mod;

/// A bridge line as a chat line: its `§` colour codes read as styles (`text_from_legacy`). The
/// stripped text is kept only for a line the engine refuses.
fn legacy(host: &dyn ServerHost, line: &str) -> Text {
    host.text_from_legacy(line)
        .unwrap_or_else(|_| Text::literal(bridge::console::strip_colors(line)))
}

/// The English line of a vanilla message the bridge takes as plain text.
/// TODO(arismoko-api: text_plain): `Text::translate(key).plain()` would give the server language's
/// text for any key; until then only these keys have a line, and a server in another language gets
/// English here.
fn vanilla_line(key: &str, name: &str) -> Option<String> {
    match key {
        "multiplayer.player.joined" => Some(format!("{name} joined the game")),
        "multiplayer.player.left" => Some(format!("{name} left the game")),
        "death.attack.generic" => Some(format!("{name} died")),
        _ => None,
    }
}

/// The death line: the cause's key when it has a line, else the generic one.
/// TODO(arismoko-api: player_died_v2): the killer's and item's names are not known here, so
/// `death.attack.player` and friends fall back to the generic line.
fn death_line(cause: Option<&str>, name: &str) -> String {
    cause
        .and_then(|key| vanilla_line(key, name))
        .or_else(|| vanilla_line("death.attack.generic", name))
        .unwrap_or_default()
}

/// Whether `uuid` holds DiscordSRV's permission `node`, whose default is `default_level` (0:
/// everyone, 2: operators). A permissions mod's answer decides when it sets one; a player who is
/// not on the server holds only the nodes that default to everyone.
fn has_node(host: &dyn ServerHost, uuid: u128, node: &str, default_level: u8) -> bool {
    let Some(player) = host.player(uuid) else {
        return default_level == 0;
    };
    if default_level == 0 {
        player.permission_value(host, node) != Some(false)
    } else {
        player.has_permission(host, node)
    }
}

fn is_fake(host: &dyn ServerHost, uuid: u128) -> bool {
    host.player(uuid)
        .and_then(|p| p.is_fake(host))
        .unwrap_or(false)
}

/// The players the bridge's tick sees: uuid, name and dimension.
fn players_of(host: &dyn ServerHost) -> Vec<bridge::Player> {
    host.players()
        .server_players()
        .into_iter()
        .map(|p| bridge::Player {
            uuid: p.uuid(),
            name: p.name(host).unwrap_or_default(),
            dimension: p
                .dimension(host)
                .map(|d| d.to_string())
                .unwrap_or_default(),
        })
        .collect()
}

/// TODO(arismoko-api: advancement_display, text_plain): the title of an advancement (a data
/// pack's too) and whether the game announces it are not readable, so nothing is announced yet.
/// The key the title would come from is built here, as the glue built it.
fn advancement_title(advancement: &str) -> Option<String> {
    let path = advancement.split_once(':').map_or(advancement, |(_, p)| p);
    if path.starts_with("recipes/") || path.ends_with("/root") {
        return None;
    }
    let _key = format!("advancements.{}.title", path.replace('/', "."));
    None
}

#[mod_api::horizon_mod]
impl Hooks for Mod {
    fn register_configs(&self, _: HostSide, event: &mut RegisterConfigsEvent<'_>) {
        let mut config = Config::new(ID, ConfigKind::Common, bridge::config::DEFAULT_CONFIG);
        config.file = CONFIG_FILE.to_string();
        event.configs.push(config);
    }

    fn server_about_to_start(&self, _: &mut dyn ServerHost, event: &mut ServerAboutToStartEvent) {
        if !event.dedicated {
            // A client or single player: nothing forwards the log, so the log feed stays closed.
            bridge::log_feed_off();
            return;
        }
        bridge::start(Path::new("config"), format!("Horizon (MC: {GAME_VERSION})"));
    }

    fn server_stopping(&self, host: &mut dyn ServerHost, event: &mut ServerStoppingEvent) {
        if !event.dedicated {
            return;
        }
        // The players still on get their leave notices first, as on Paper; then the bridge sends
        // the shutdown message and topics and disconnects.
        if bridge::chat_running() {
            for player in host.players().server_players() {
                let name = player.name(&*host).unwrap_or_default();
                let line = vanilla_line("multiplayer.player.left", &name).unwrap_or_default();
                bridge::player_leave(player.uuid(), &name, &line, false);
            }
        }
        bridge::stop();
    }

    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        if !matches!(
            event.phase,
            ServerTickPhase::ServerTickStart {
                world_running: true,
                ..
            }
        ) {
            return;
        }
        let (rate, mspt) = host.tick_stats();
        let tps = if mspt > 0.0 {
            rate.min((1000.0 / mspt) as f32)
        } else {
            rate
        };
        let work = bridge::tick(tps, || players_of(&*host));
        for command in work.commands {
            host.commands().run_command(&command);
        }
        for command in work.console {
            // The console channel gets the command's own answer lines, as the console prints them.
            let (ran, output) = host.server_run_command_captured(&command);
            bridge::console_output(&command, ran, output);
        }
        for line in work.broadcasts {
            let text = legacy(&*host, &line);
            let _ = host.messages().broadcast(text);
        }
        for (uuid, line) in work.tells {
            if let Some(player) = host.player(uuid) {
                let text = legacy(&*host, &line);
                let _ = player.tell(host, text);
            }
        }
    }

    fn console_line(&self, _: &mut dyn ServerHost, event: &mut ConsoleLineEvent<'_>) {
        // Every line the server logs goes to the console channel (lines a listener logs itself are
        // not heard again).
        bridge::log_line(event.level, event.message);
    }

    fn player_login(&self, _: &mut dyn ServerHost, event: &mut PlayerLoginEvent<'_>) -> Flow {
        // The bridge looks for the player's file to tell a first join. It refuses nobody.
        bridge::player_login(event.player);
        Flow::Continue
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        if is_fake(&*host, event.player) {
            bridge::fake_joined(event.player);
            return;
        }
        let message = vanilla_line("multiplayer.player.joined", &event.name).unwrap_or_default();
        // discordsrv.silentjoin, default operator in DiscordSRV's plugin.yml.
        let silent = has_node(&*host, event.player, "discordsrv.silentjoin", 2);
        bridge::player_join(event.player, &event.name, &message, silent);
    }

    fn player_logged_out(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        if bridge::fake_left(event.player) {
            return;
        }
        let message = vanilla_line("multiplayer.player.left", &event.name).unwrap_or_default();
        // discordsrv.silentquit, default operator. The player is already gone from the list here,
        // so the node reads as unheld unless the server is stopping (see PORT_NOTES.md).
        let silent = has_node(&*host, event.player, "discordsrv.silentquit", 2);
        bridge::player_leave(event.player, &event.name, &message, silent);
    }

    fn player_died(&self, host: &mut dyn ServerHost, event: &mut PlayerDiedEvent<'_>) {
        if bridge::is_fake(event.player) {
            return;
        }
        let live = host.player(event.player).and_then(|p| {
            let name = p.name(&*host)?;
            let dimension = p.dimension(&*host)?.to_string();
            Some((name, dimension))
        });
        let known =
            live.or_else(|| bridge::known_player(event.player).map(|p| (p.name, p.dimension)));
        let Some((name, dimension)) = known else {
            return;
        };
        let message = death_line(event.cause.as_deref(), &name);
        bridge::player_death(event.player, &name, &dimension, &message);
    }

    fn player_advancement_done(
        &self,
        _: &mut dyn ServerHost,
        event: &mut PlayerAdvancementDoneEvent<'_>,
    ) {
        let advancement: &str = &event.advancement;
        let Some(title) = advancement_title(advancement) else {
            return;
        };
        let dimension = bridge::known_player(event.player)
            .map(|p| p.dimension)
            .unwrap_or_default();
        bridge::player_advancement(event.player, &event.name, &dimension, &title);
    }

    fn server_chat(&self, _: &mut dyn ServerHost, event: &mut ServerChatEvent<'_>) -> Flow {
        // A cancelled message (by a mod before this one) is not told to listeners at all.
        if !event.cancelled {
            bridge::player_chat(event.player, &event.username, &event.raw_text);
        }
        Flow::Continue
    }

    fn player_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerCommandEvent<'_>,
    ) -> Flow {
        let mut words = event.command.splitn(2, ' ');
        let is_discord = words.next().is_some_and(|root| {
            root.eq_ignore_ascii_case("discord") || root.eq_ignore_ascii_case("discordsrv")
        });
        if !is_discord {
            return Flow::Continue;
        }
        let player = event.player;
        let has = |node: &str, default_level: u8| has_node(&*host, player, node, default_level);
        let args = words.next().unwrap_or("");
        match bridge::discord_command(event.player, &event.name, args, &has) {
            Some(lines) => {
                event.cancelled = true;
                event.replies.extend(lines.iter().map(|line| legacy(&*host, line)));
                Flow::Cancel
            }
            // The bridge is off (no token, no channel): answer, do not fall through to "unknown command".
            None => {
                event.cancelled = true;
                // Staff sub-commands get DiscordSRV's permission line first, as the bridge gives it when it runs.
                let mut parts = args.split_whitespace();
                let sub = parts.next().unwrap_or("").to_ascii_lowercase();
                // `link <player> <discord>` and `linked <target>` act on other players: staff too.
                let staff = [
                    "broadcast", "bcast", "reload", "resync", "unlink", "clearlinked", "language",
                    "setlanguage", "lang", "setlang", "debugger", "debug",
                ]
                .contains(&sub.as_str())
                    || (sub == "link" && parts.clone().count() >= 1 && args.split_whitespace().count() > 1)
                    || (sub == "linked" && parts.clone().count() >= 1);
                if staff && !has("discordsrv.discord", 2) {
                    event.replies.push(legacy(&*host, "§cYou do not have permission to perform this command."));
                    return Flow::Cancel;
                }
                event.replies.push(Text::literal(
                    "DiscordSRV is not connected on this server (it is switched off: no bot token, no channel).",
                ));
                Flow::Cancel
            }
        }
    }

    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        // Registered even when the bridge is off, so the commands answer with the line above.
        // One node covers the whole root (`Command::with_node`): every usage names
        // `discordsrv.discord`. Each usage keeps its level as the fallback when no permissions mod
        // answers, so the staff usages stay operator-only as before (DiscordSRV's `discordsrv.*`
        // nodes default to op). The per-usage DiscordSRV nodes (`discordsrv.link.others`, ...) are
        // checked inside the bridge's command handling.
        // NOTE: `link <player>` takes a word, not a player argument, so an offline player can be
        // linked as DiscordSRV allows.
        const USAGES: [(&str, u8); 19] = [
            ("", 0),
            (" help [command:word]", 0),
            (" link", 0),
            (" link <player:word> <discord:text>", 2),
            (" linked", 0),
            (" linked <target:text>", 2),
            (" unlink", 2),
            (" unlink <target:text>", 2),
            (" clearlinked", 2),
            (" clearlinked <target:text>", 2),
            (" broadcast <message:text>", 2),
            (" bcast <message:text>", 2),
            (" resync", 2),
            (" reload", 2),
            (" language [language:word]", 2),
            (" setlanguage [language:word]", 2),
            (" lang [language:word]", 2),
            (" setlang [language:word]", 2),
            (" debugger [action:word] [categories:text]", 2),
        ];
        for root in ["discord", "discordsrv"] {
            for (args, level) in USAGES {
                event.commands.push(
                    Command::op(&format!("{root}{args}"), level).with_node("discordsrv.discord"),
                );
            }
        }
    }
}
