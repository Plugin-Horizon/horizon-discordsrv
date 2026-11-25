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
//!   call these functions. `set_console_preprocessor` keeps only that one; `add_console_preprocessor(f)`
//!   adds one after the others and returns a `PreprocessorHandle` for `remove_console_preprocessor`. They
//!   run in order, each gets the command the one before returned, the first `Refuse` ends the chain.
//! - `enum Preprocess { Run(String), Refuse { reply: String } }`: `Run` runs that command (the
//!   message as it came, or another one), `Refuse` runs nothing and posts `reply` to the channel at once.
//!
//! Until a preprocessor is set, a console channel message runs as it came.

pub mod discordsrv_bridge;
use discordsrv_bridge as bridge;
use mod_api::{
    Command, CommandArgumentValue, CommandSource, ConsoleCommandEvent, ConsoleLineEvent, Config, ConfigKind, Flow, Hooks, HostSide,
    NodeDefault, PermissionGatherNodesEvent, PermissionLevel, PermissionNode, PermissionValueType,
    BanChangedEvent, BanListKind, ModServiceCallEvent, PermissionGroupsChangedEvent, PlayerAdvancementDoneEvent, PlayerCommandEvent, PlayerDeathMessageEvent, PlayerLoggedInEvent,
    PlayerLoggedOutEvent, PlayerLoginEvent, Identifier, RegisterNodeCommandsEvent, RegisterConfigsEvent,
    RunModCommandEvent, ServerAboutToStartEvent, ServerChatEvent, ServerHost, ServerStoppingEvent, ServerTickEvent,
    ServerTickPhase, Text,
};
use mod_api::text::TextContent;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::path::Path;

pub use crate::discordsrv_bridge::{
    Preprocess, Preprocessor, PreprocessorHandle, add_console_preprocessor, remove_console_preprocessor, set_console_preprocessor,
};

const ID: &str = "discordsrv";
/// Relative to the server's `config/` folder.
const CONFIG_FILE: &str = "discordsrv/config.toml";
const GAME_VERSION: &str = "26.3";

#[derive(Default)]
struct Mod {
    /// Players who held `discordsrv.silentquit` when they joined: the node is asked at login,
    /// because the player is already gone from the server when the quit hook runs.
    silent_quit: Mutex<BTreeSet<u128>>,
    /// Ticks seen, to sample the players' positions every `Tick speed` ticks for the voice module.
    voice_ticks: std::sync::atomic::AtomicU64,
    /// Ticks seen, to read what other mods say about the players about once a second.
    extras_ticks: std::sync::atomic::AtomicU64,
    /// The `discordsrv.sync.<group>` nodes are declared (once, after the bridge has read its config).
    sync_nodes_declared: std::sync::atomic::AtomicBool,
    /// Players a vanish mod hid when they joined: their quit is not announced either.
    vanished_join: Mutex<BTreeSet<u128>>,
}

/// A bridge line as a chat line: its `§` colour codes read as styles (`text_from_legacy`). The
/// stripped text is kept only for a line the engine refuses.
fn legacy(host: &dyn ServerHost, line: &str) -> Text {
    host.text_from_legacy(line)
        .unwrap_or_else(|_| Text::literal(bridge::console::strip_colors(line)))
}

/// The English line of a vanilla message the bridge takes as plain text.
/// NOTE: `Text::plain` has no language file, so keys come from the embedded en_us table
/// (the table below, and names made from `entity.minecraft.*`, `item.minecraft.*`, `block.minecraft.*` keys); a server in another language gets English here.
fn vanilla_line(key: &str, name: &str) -> Option<String> {
    let text = match key {
        "multiplayer.player.joined" => "{name} joined the game",
        "multiplayer.player.left" => "{name} left the game",
        // The death messages the game builds without a killer (its key comes with the death).
        "death.attack.generic" => "{name} died",
        "death.attack.lava" => "{name} tried to swim in lava",
        "death.attack.drown" => "{name} drowned",
        "death.attack.fall" => "{name} hit the ground too hard",
        "death.attack.starve" => "{name} starved to death",
        "death.attack.cactus" => "{name} was pricked to death",
        "death.attack.explosion" => "{name} blew up",
        "death.attack.onFire" => "{name} burned to death",
        "death.attack.inFire" => "{name} went up in flames",
        "death.attack.inWall" => "{name} suffocated in a wall",
        "death.attack.outOfWorld" => "{name} fell out of the world",
        "death.attack.magic" => "{name} was killed by magic",
        "death.attack.wither" => "{name} withered away",
        "death.attack.anvil" => "{name} was squashed by a falling anvil",
        "death.attack.fallingBlock" => "{name} was squashed by a falling block",
        "death.attack.dragonBreath" => "{name} was roasted in dragon breath",
        "death.attack.freeze" => "{name} froze to death",
        "death.attack.sweetBerryBush" => "{name} was poked to death by a sweet berry bush",
        "death.attack.stalagmite" => "{name} was impaled on a stalagmite",
        "death.attack.lightningBolt" => "{name} was struck by lightning",
        // The killer's kinds: `%2$s` is the killer, `%3$s` the weapon (the arguments of the built text).
        "death.attack.mob" => "{name} was slain by %2$s",
        "death.attack.mob.item" => "{name} was slain by %2$s using %3$s",
        "death.attack.player" => "{name} was slain by %2$s",
        "death.attack.player.item" => "{name} was slain by %2$s using %3$s",
        "death.attack.arrow" => "{name} was shot by %2$s",
        "death.attack.arrow.item" => "{name} was shot by %2$s using %3$s",
        "death.attack.fireball" => "{name} was fireballed by %2$s",
        "death.attack.fireball.item" => "{name} was fireballed by %2$s using %3$s",
        "death.attack.thrown" => "{name} was pummeled by %2$s",
        "death.attack.thrown.item" => "{name} was pummeled by %2$s using %3$s",
        "death.attack.explosion.player" => "{name} was blown up by %2$s",
        "death.attack.indirectMagic" => "{name} was killed by %2$s using magic",
        "death.attack.indirectMagic.item" => "{name} was killed by %2$s using %3$s",
        "death.attack.thorns" => "{name} was killed trying to hurt %2$s",
        "death.attack.sting" => "{name} was stung to death by %2$s",
        "death.attack.cramming" => "{name} was squished too much",
        "death.attack.flyIntoWall" => "{name} experienced kinetic energy",
        "death.attack.hotFloor" => "{name} discovered the floor was lava",
        "death.attack.fallingStalactite" => "{name} was skewered by a falling stalactite",
        "death.attack.badRespawnPoint.message" => "{name} was killed by %2$s",
        _ => return None,
    };
    Some(text.replace("{name}", name))
}

/// The English template of a translation key, with `%1$s`.. placeholders: the keys of
/// [`vanilla_line`], and a name made from the key for `entity.minecraft.*`, `item.minecraft.*` and
/// `block.minecraft.*` (`zombie_villager` -> `Zombie Villager`). No Mojang language file is shipped.
fn template(key: &str) -> Option<String> {
    for prefix in ["entity.minecraft.", "item.minecraft.", "block.minecraft."] {
        if let Some(id) = key.strip_prefix(prefix) {
            if id.contains('.') {
                return None;
            }
            let words: Vec<String> = id
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
                })
                .collect();
            return Some(words.join(" "));
        }
    }
    vanilla_line(key, "%1$s")
}

/// A text as plain characters: literals, and translation keys through [`lang`] (the fallback, else the key).
fn plain_text(text: &Text) -> String {
    plain_text_with(text, &|_| None)
}

/// The server's own language text for a key (`ServerHost::translate`).
fn host_translate(host: &dyn ServerHost) -> impl Fn(&str) -> Option<String> + '_ {
    move |key| host.translate(key)
}

/// [`plain_text`] with `tr` asked first for each translation key (then the table, the fallback, the key).
fn plain_text_with(text: &Text, tr: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = match &text.content {
        TextContent::Literal(s) => s.clone(),
        TextContent::Translatable { key, fallback, arguments } => {
            let mut line = tr(key).or_else(|| template(key)).or_else(|| fallback.clone()).unwrap_or_else(|| key.clone());
            for (i, arg) in arguments.iter().enumerate() {
                let value = plain_text_with(arg, tr);
                line = line.replace(&format!("%{}$s", i + 1), &value);
                if let Some(at) = line.find("%s") {
                    line.replace_range(at..at + 2, &value);
                }
            }
            line
        }
        _ => String::new(),
    };
    for child in &text.children {
        out.push_str(&plain_text_with(child, tr));
    }
    out
}

#[allow(dead_code)]
/// The death line: the cause's key when it has a line, else the generic one.
/// The English texts are vanilla's `en_us` lines for the keys listed in [`vanilla_line`]. A key
/// without a line (the killer's kinds, `death.attack.player` and friends) falls back to the generic line.
/// Only used for a death without a built message; the real line comes from `player_death_message`.
fn death_line(cause: Option<&str>, name: &str) -> String {
    cause
        .and_then(|key| vanilla_line(key, name))
        .filter(|line| !line.contains("%2$s"))
        .or_else(|| vanilla_line("death.attack.generic", name))
        .unwrap_or_default()
}

/// The level of a node that nobody has by default (DiscordSRV's `default: false`).
const NOBODY: u8 = u8::MAX;

/// Every permission node the bridge asks, with the operator level it defaults to (0: everyone, 2:
/// operators, `NOBODY`: nobody). Declared in `permission_gather_nodes`; a node not listed here is
/// answered as denied. DiscordSRV's `plugin.yml` has the same defaults (`default: true`, `op`, `false`).
const NODES: [(&str, u8); 19] = [
    ("discordsrv.bcast", 2),
    ("discordsrv.debug", 2),
    ("discordsrv.help", 0),
    ("discordsrv.language", 2),
    ("discordsrv.link", 0),
    ("discordsrv.linked", 0),
    ("discordsrv.reload", 2),
    ("discordsrv.resync", 2),
    ("discordsrv.unlink", 2),
    ("discordsrv.link.others", 2),
    ("discordsrv.linked.others", 2),
    ("discordsrv.silentjoin", NOBODY),
    ("discordsrv.silentquit", NOBODY),
    ("discordsrv.discord", 0),
    ("discordsrv.chat", 0),
    ("discordsrv.unlink.others", 2),
    ("discordsrv.nicknamesync", 0),
    ("discordsrv.updatenotification", 2),
    ("discordsrv.groupsyncwithcommands", 2),
];

/// The default of a declared node: its operator level, or nobody (`NOBODY`, Bukkit's `default: false`).
fn node_default(level: u8) -> NodeDefault {
    if level == NOBODY {
        NodeDefault::Nobody
    } else {
        NodeDefault::Level(PermissionLevel::from_level(i32::from(level)))
    }
}

/// A Boolean permission node named `name`.
fn node(name: &str) -> PermissionNode {
    PermissionNode::new(name, PermissionValueType::Boolean)
}

/// Whether `uuid` holds DiscordSRV's permission `node`. A player who is not on the server holds
/// only the nodes that default to everyone (`default_level` 0). Any handler error is a denial.
fn has_node(host: &mut dyn ServerHost, uuid: u128, node_name: &str, default_level: u8) -> bool {
    let Some(player) = host.player(uuid) else {
        return default_level == 0;
    };
    player.permission_bool(host, &node(node_name)).unwrap_or(false)
}

/// The vanilla whitelist and the name ban list for a player: the two facts `Require linked account to
/// play` asks of the server. An ended ban that the list still holds does not count.
fn login_facts(host: &dyn ServerHost, uuid: u128) -> (bool, bool) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let banned = host
        .server_ban_list(BanListKind::Players)
        .iter()
        .any(|b| b.uuid == Some(uuid) && b.expires_millis.is_none_or(|t| t > now));
    (host.server_is_whitelisted(uuid), banned)
}

/// A game event for alerts.yml (does nothing without alerts): the player's facts and the permission
/// question are gathered here, where the host is.
fn alert(host: &mut dyn ServerHost, kind: bridge::alerts::Kind, uuid: Option<u128>, message: &str, cancelled: bool) {
    if !bridge::alerts_on() {
        return;
    }
    let (facts, ping, known) = match uuid.and_then(|u| host.player(u).map(|p| (u, p))) {
        Some((u, p)) => {
            let dimension = p.dimension(&*host).map(|d| d.to_string()).unwrap_or_default();
            let mode = match p.game_mode(&*host).map(|m| m.id()) {
                Some(1) => "CREATIVE",
                Some(2) => "ADVENTURE",
                Some(3) => "SPECTATOR",
                _ => "SURVIVAL",
            };
            let name = p.name(&*host).unwrap_or_default();
            let ping = p.latency(&*host).unwrap_or(0) as i64;
            let facts = bridge::alerts::PlayerFacts {
                name: name.clone(),
                uuid: bridge::server::uuid_string(u),
                world: bridge::world_name(&dimension),
                game_mode: mode.to_string(),
                ping,
            };
            (Some(facts), ping, Some(bridge::server::Player { uuid: u, name, dimension }))
        }
        None => match uuid.and_then(|u| bridge::known_player(u).map(|k| (u, k))) {
            // A player who just left: what the last snapshot knew.
            Some((u, k)) => {
                let facts = bridge::alerts::PlayerFacts {
                    name: k.name.clone(),
                    uuid: bridge::server::uuid_string(u),
                    world: bridge::world_name(&k.dimension),
                    game_mode: "SURVIVAL".to_string(),
                    ping: 0,
                };
                (Some(facts), 0, Some(k))
            }
            None => (None, 0, None),
        },
    };
    let online: Vec<String> = host
        .players()
        .server_players()
        .into_iter()
        .filter_map(|p| p.name(&*host))
        .collect();
    let cell = std::cell::RefCell::new(host);
    let perm = |node: &str| uuid.is_some_and(|u| has_node(&mut **cell.borrow_mut(), u, node, 4));
    let ev = bridge::alerts::Event { kind, player: facts.as_ref(), message, cancelled, online, has_permission: &perm };
    bridge::alert_event(&ev, known, ping);
}

fn lock_set(set: &Mutex<BTreeSet<u128>>) -> std::sync::MutexGuard<'_, BTreeSet<u128>> {
    set.lock().unwrap_or_else(|e| e.into_inner())
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

/// Reads what other mods say about the online players, for the bridge: the primary group the permissions
/// provider published (Vault's `getPrimaryGroup`), whether a vanish mod hides them (the engine's `hide`, which
/// SuperVanish uses), and the alias a world mod gives each dimension (Multiverse's `multiverse:world_alias`,
/// which answers nothing when that mod is not loaded).
fn publish_extras(host: &mut dyn ServerHost) {
    let mut extras = HashMap::new();
    let mut aliases: HashMap<String, String> = HashMap::new();
    let players = host.players().server_players();
    for p in players {
        let uuid = p.uuid();
        let primary_group = if bridge::hook_enabled("Vault") { host.permission_groups(uuid).primary.unwrap_or_default() } else { String::new() };
        let vanished = bridge::hook_enabled("SuperVanish") && p.hidden(&*host).is_some_and(|h| h.is_some());
        extras.insert(uuid, bridge::Extra { primary_group, vanished });
        let Some(dimension) = p.dimension(&*host).map(|d| d.to_string()) else { continue };
        if aliases.contains_key(&dimension) {
            continue;
        }
        let world = bridge::world_name(&dimension);
        if bridge::hook_enabled("Multiverse-Core")
            && let Ok(reply) = host.commands().call_mod_service("multiverse:world_alias", world.as_bytes())
            && let Ok(alias) = String::from_utf8(reply)
            && !alias.trim().is_empty()
        {
            aliases.insert(dimension, alias);
        }
    }
    bridge::set_player_extras(extras, aliases);
}

/// What the group synchronisation sees of a player: the groups the permissions provider published, plus a group for
/// each `discordsrv.sync.<group>` node they hold, minus each one `discordsrv.sync.deny.<group>` denies (when
/// `GroupRoleSynchronizationEnableDenyPermission` is on), as DiscordSRV reads them through Vault.
fn groups_of(host: &mut dyn ServerHost, uuid: u128) -> bridge::sync::Groups {
    let published = host.permission_groups(uuid);
    let (mut all, mut primary) = (published.groups, published.primary);
    let (names, deny) = bridge::sync_group_names();
    for group in names {
        if has_node(&mut *host, uuid, &format!("discordsrv.sync.{group}"), NOBODY) && !all.iter().any(|g| g.eq_ignore_ascii_case(&group)) {
            all.push(group.clone());
        }
        if deny && has_node(&mut *host, uuid, &format!("discordsrv.sync.deny.{group}"), NOBODY) {
            all.retain(|g| !g.eq_ignore_ascii_case(&group));
            if primary.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&group)) {
                primary = None;
            }
        }
    }
    bridge::sync::Groups { all, primary }
}

/// Declares `discordsrv.sync.<group>` (and `.deny.`) for every synchronised group: nobody holds them unless granted.
fn declare_sync_nodes(host: &mut dyn ServerHost) {
    let (names, deny) = bridge::sync_group_names();
    for group in names {
        let _ = host.permission_declare_node(&node(&format!("discordsrv.sync.{group}")), &NodeDefault::Nobody);
        if deny {
            let _ = host.permission_declare_node(&node(&format!("discordsrv.sync.deny.{group}")), &NodeDefault::Nobody);
        }
    }
}

/// Discord -> Minecraft group synchronisation: gives or takes a group through the permissions provider.
/// Horizon has no provider-neutral write (ENGINE_NEEDS `permission_group_write`), so this asks the provider's
/// console command (LuckPerms' syntax). True when the command ran.
fn group_write(host: &mut dyn ServerHost, uuid: u128, group: &str, add: bool) -> bool {
    if group.is_empty() || !group.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return false;
    }
    let command = format!("lp user {} parent {} {group}", bridge::server::uuid_string(uuid), if add { "add" } else { "remove" });
    host.commands().run_command_with_output(&command).success
}

/// The players with their positions, for the voice module: world, block position and whether they are alive.
fn voice_players_of(host: &dyn ServerHost) -> Vec<(u128, bridge::voice::VoicePlayer)> {
    host.players()
        .server_players()
        .into_iter()
        .filter(|p| !p.is_fake(host).unwrap_or(false))
        .filter_map(|p| {
            let entity = p.entity();
            let pos = entity.position(host)?;
            let alive = host.living(entity).and_then(|l| l.health(host)).is_none_or(|h| h > 0.0);
            let world = p.dimension(host).map(|d| d.to_string()).unwrap_or_default();
            Some((p.uuid(), bridge::voice::VoicePlayer { world, pos, alive }))
        })
        .collect()
}

/// The title of an announced advancement: None for a hidden one (no display) or one the game does
/// not announce in chat (`announce_to_chat` false, recipes, roots).
fn advancement_title(host: &mut dyn ServerHost, advancement: &str) -> Option<String> {
    let id = Identifier::parse(advancement).ok()?;
    let display = host.advancement(&id)?.display?;
    if !display.announce_to_chat {
        return None;
    }
    let tr = host_translate(&*host);
    Some(plain_text_with(&display.title, &tr))
}

/// The answer to `/discord <args>`. `sender` is the player (uuid, name); None is the console (or a
/// command block), which holds every node.
fn discord_answer(host: &mut dyn ServerHost, sender: Option<(u128, &str)>, args: &str) -> Vec<Text> {
    let (uuid, name) = sender.unwrap_or((bridge::CONSOLE, "CONSOLE"));
    // The answers are read once here: the bridge's `has` is a plain `Fn`, and a query needs `&mut` host.
    let granted: BTreeMap<&str, bool> = NODES
        .iter()
        .map(|&(node_name, level)| (node_name, sender.is_none() || has_node(&mut *host, uuid, node_name, level)))
        .collect();
    let has = |node_name: &str, _default_level: u8| granted.get(node_name).copied().unwrap_or(false);
    match bridge::discord_command(uuid, name, args, &has) {
        Some(lines) => lines.iter().map(|line| legacy(&*host, line)).collect(),
        // The bridge is off (no token, no channel): answer, do not fall through to "unknown command".
        None => {
            // A sub-command the sender may not run gets DiscordSRV's permission line first, as the
            // bridge gives it when it runs.
            let mut parts = args.split_whitespace();
            let sub = parts.next().unwrap_or("").to_ascii_lowercase();
            let given = parts.count();
            let mut needed: Vec<&str> = Vec::new();
            if let Some((node_name, _)) = bridge::command_permission(&sub) {
                needed.push(node_name);
            }
            match sub.as_str() {
                "link" if given >= 2 => needed.push("discordsrv.link.others"),
                "linked" | "unlink" | "clearlinked" if given >= 1 => needed.push("discordsrv.linked.others"),
                _ => {}
            }
            if needed.iter().any(|node_name| !has(node_name, 0)) {
                return vec![legacy(&*host, "§cYou do not have permission to perform this command.")];
            }
            vec![Text::literal(
                "DiscordSRV is not connected on this server (it is switched off: no bot token, no channel).",
            )]
        }
    }
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
            let players = host.players().server_players();
            for player in players {
                let name = player.name(&*host).unwrap_or_default();
                let line = vanilla_line("multiplayer.player.left", &name).unwrap_or_default();
                let silent = has_node(&mut *host, player.uuid(), "discordsrv.silentquit", NOBODY);
                bridge::player_leave(player.uuid(), &name, &line, silent);
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
        if let Some(every) = bridge::voice_every()
            && self.voice_ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % every == 0
        {
            bridge::voice_players(voice_players_of(&*host));
        }
        if bridge::chat_running() && !self.sync_nodes_declared.swap(true, std::sync::atomic::Ordering::Relaxed) {
            declare_sync_nodes(&mut *host);
        }
        if self.extras_ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 20 == 0 && bridge::chat_running() {
            publish_extras(&mut *host);
        }
        let mut work = bridge::tick(tps, || players_of(&*host));
        for command in work.commands {
            host.commands().run_command(&command);
        }
        for command in work.console {
            // The console channel gets the command's own answer lines, as the console prints them.
            let result = host.commands().run_command_with_output(&command);
            bridge::console_output(&command, result.success, result.feedback);
        }
        for (uuid, line) in work.kicks {
            // The unlink kick has the link requirement's exceptions (whitelist, "only banned players").
            let (whitelisted, banned) = login_facts(&*host, uuid);
            if !bridge::unlink_kick_applies(whitelisted, banned) {
                continue;
            }
            if let Some(player) = host.player(uuid) {
                let text = legacy(&*host, &line);
                let _ = player.disconnect(host, text);
            }
        }
        // Group <-> role synchronisation: the groups are read here, the Discord calls run on the chat thread.
        let mut batches: Vec<(bridge::sync::Direction, Vec<u128>)> = Vec::new();
        if work.resync_all || !work.resync.is_empty() {
            let uuids = if work.resync_all { bridge::linked_uuids() } else { work.resync.clone() };
            batches.push((bridge::sync::Direction::Authoritative, uuids));
        }
        for (uuids, nicks) in [(std::mem::take(&mut work.resync_cycle), false), (std::mem::take(&mut work.resync_nicknames), true)] {
            if uuids.is_empty() {
                continue;
            }
            let mut players = HashMap::new();
            for uuid in uuids {
                players.insert(uuid, groups_of(&mut *host, uuid));
            }
            if nicks {
                bridge::resync_nicknames(players);
            } else {
                bridge::resync_groups_only(players);
            }
        }
        if !work.resync_to_minecraft.is_empty() {
            batches.push((bridge::sync::Direction::ToMinecraft, work.resync_to_minecraft.clone()));
        }
        for (direction, uuids) in batches {
            let mut players = HashMap::new();
            for uuid in uuids {
                players.insert(uuid, groups_of(&mut *host, uuid));
            }
            bridge::resync(players, direction);
        }
        for (uuid, group, add) in work.group_writes {
            if !group_write(&mut *host, uuid, &group, add) {
                log::warn!(
                    target: "horizon_discordsrv",
                    "[DiscordSRV] Group synchronization: the permissions provider did not {} the group {group}",
                    if add { "give" } else { "take" }
                );
            }
        }
        for name in work.mentions {
            // MinecraftMentionSound: the note-block pling, to the mentioned player only.
            host.commands().run_command(&format!("playsound minecraft:block.note_block.pling master {name} ~ ~ ~ 1 1"));
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

    /// DiscordSRV's LuckPerms hook (instant group updates): a group change on the server updates the roles at once.
    fn permission_groups_changed(&self, _: &mut dyn ServerHost, event: &mut PermissionGroupsChangedEvent<'_>) {
        // Fired inside the provider's own publish call: the published groups are used as they are and no permission
        // node is asked here (the provider may be mid-update); `discordsrv.sync.<group>` nodes count at the next cycle.
        bridge::groups_changed(event.player, bridge::sync::Groups { all: event.new.groups.clone(), primary: event.new.primary.clone() });
    }

    /// The services other mods can call (`host.commands().call_mod_service`):
    /// - `discordsrv:placeholder`, payload `<uuid as 32 hex digits, or -> <identifier>`: the value of `%discordsrv_<identifier>%`
    ///   (DiscordSRV's PlaceholderAPI expansion), reply the text; unknown for a player: unhandled.
    /// - `discordsrv:contexts`, payload `<uuid>`: LuckPerms' contexts, reply `key=value` lines.
    /// - `discordsrv:fake_join` / `discordsrv:fake_quit`, payload `<uuid> <message>`: a vanish mod's fake join or quit
    ///   message is announced on Discord (SuperVanish's `PostPlayerShowEvent` / `PostPlayerHideEvent`).
    fn mod_service_call(&self, host: &mut dyn ServerHost, event: &mut ModServiceCallEvent<'_>) -> Option<mod_api::PayloadBytes> {
        let text = std::str::from_utf8(&event.payload.0).ok()?;
        let (who, rest) = text.split_once(' ').unwrap_or((text, ""));
        let uuid = if who == "-" { None } else { bridge::server::parse_uuid(who) };
        match event.service {
            "discordsrv:placeholder" => bridge::placeholder(uuid, rest.trim()).map(|v| mod_api::PayloadBytes(v.into_bytes())),
            "discordsrv:contexts" => {
                let lines: Vec<String> = bridge::contexts(uuid?).into_iter().map(|(k, v)| format!("{k}={v}")).collect();
                Some(mod_api::PayloadBytes(lines.join("\n").into_bytes()))
            }
            "discordsrv:fake_join" | "discordsrv:fake_quit" => {
                let uuid = uuid?;
                let join = event.service == "discordsrv:fake_join";
                let name = host.player(uuid).and_then(|p| p.name(&*host)).or_else(|| bridge::known_player(uuid).map(|p| p.name))?;
                let node = if join { "discordsrv.silentjoin" } else { "discordsrv.silentquit" };
                let silent = has_node(&mut *host, uuid, node, NOBODY);
                if join {
                    bridge::player_join(uuid, &name, rest, silent);
                } else {
                    bridge::player_leave(uuid, &name, rest, silent);
                }
                Some(mod_api::PayloadBytes(b"ok".to_vec()))
            }
            _ => None,
        }
    }

    fn console_line(&self, _: &mut dyn ServerHost, event: &mut ConsoleLineEvent<'_>) {
        // Every line the server logs goes to the console channel (lines a listener logs itself are
        // not heard again).
        bridge::log_line(event.level, event.message);
    }

    fn player_login(&self, host: &mut dyn ServerHost, event: &mut PlayerLoginEvent<'_>) -> Flow {
        // The bridge looks for the player's file to tell a first join.
        bridge::player_login(event.player);
        // `Require linked account to play` (linking.yml): an unlinked player is refused with the message.
        let (whitelisted, banned) = if bridge::login_check_on() { login_facts(&*host, event.player) } else { (false, false) };
        if let Some(message) = bridge::login_refusal(event.player, event.name, whitelisted, banned) {
            let text = bridge::format::translate_ampersand(&message);
            *event.reason = Some(legacy(&*host, &text));
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        if is_fake(&*host, event.player) {
            bridge::fake_joined(event.player);
            return;
        }
        let message = vanilla_line("multiplayer.player.joined", &event.name).unwrap_or_default();
        // discordsrv.silentjoin, default nobody in DiscordSRV's plugin.yml.
        let silent = has_node(&mut *host, event.player, "discordsrv.silentjoin", NOBODY);
        if has_node(&mut *host, event.player, "discordsrv.silentquit", NOBODY) {
            lock_set(&self.silent_quit).insert(event.player);
        }
        bridge::minecraft_unban_on_join(event.player);
        // A vanish mod hides them: no join message (PlayerJoinLeaveListener), and no quit message later.
        let vanished = bridge::hook_enabled("SuperVanish")
            && host.player(event.player).and_then(|p| p.hidden(&*host)).is_some_and(|h| h.is_some());
        if vanished {
            lock_set(&self.vanished_join).insert(event.player);
        } else {
            bridge::player_join(event.player, &event.name, &message, silent);
        }
        alert(host, bridge::alerts::Kind::Join, Some(event.player), &message, false);
    }

    fn player_logged_out(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        if bridge::fake_left(event.player) {
            return;
        }
        let message = vanilla_line("multiplayer.player.left", &event.name).unwrap_or_default();
        // discordsrv.silentquit, default nobody: the answer from login (the player is already gone here).
        let silent = lock_set(&self.silent_quit).remove(&event.player);
        let vanished = lock_set(&self.vanished_join).remove(&event.player) || bridge::is_vanished(event.player);
        if !vanished {
            bridge::player_leave(event.player, &event.name, &message, silent);
        }
        alert(host, bridge::alerts::Kind::Quit, Some(event.player), &message, false);
    }

    /// The death message the game built (killer, weapon and all), as DiscordSRV's
    /// `PlayerDeathEvent#getDeathMessage`: a death without a message (none shown) is not announced.
    fn player_death_message(&self, host: &mut dyn ServerHost, event: &mut PlayerDeathMessageEvent<'_>) {
        if bridge::is_fake(event.player) {
            return;
        }
        let Some(text) = event.message.as_ref() else {
            return;
        };
        let message = {
            let tr = host_translate(&*host);
            plain_text_with(text, &tr)
        };
        if message.trim().is_empty() {
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
        bridge::player_death(event.player, &name, &dimension, &message);
        alert(host, bridge::alerts::Kind::Death, Some(event.player), &message, false);
    }

    /// `BanSynchronizationMinecraftToDiscord`: a player ban bans the linked Discord account.
    fn ban_changed(&self, _: &mut dyn ServerHost, event: &mut BanChangedEvent) {
        if event.kind == BanListKind::Players
            && let Some(uuid) = event.record.uuid
        {
            bridge::minecraft_ban(uuid, event.added);
        }
    }

    fn player_advancement_done(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerAdvancementDoneEvent<'_>,
    ) {
        let advancement: &str = &event.advancement;
        let Some(title) = advancement_title(&mut *host, advancement) else {
            return;
        };
        let dimension = bridge::known_player(event.player)
            .map(|p| p.dimension)
            .unwrap_or_default();
        bridge::player_advancement(event.player, &event.name, &dimension, &title);
        alert(host, bridge::alerts::Kind::Advancement, Some(event.player), &title, false);
    }

    /// `ServerCommandEvent` for alerts.yml: a line typed in the console, RCON or the Discord console channel.
    fn console_command(&self, host: &mut dyn ServerHost, event: &mut ConsoleCommandEvent<'_>) -> Flow {
        alert(host, bridge::alerts::Kind::ConsoleCommand, None, event.line, event.cancelled);
        Flow::Continue
    }

    fn server_chat(&self, host: &mut dyn ServerHost, event: &mut ServerChatEvent<'_>) -> Flow {
        // A cancelled message (by a mod before this one) is not told to listeners at all; a
        // player without `discordsrv.chat` (default everyone) is not relayed to Discord.
        if !event.cancelled && has_node(&mut *host, event.player, "discordsrv.chat", 0) {
            bridge::player_chat(event.player, &event.username, &event.raw_text);
        }
        alert(host, bridge::alerts::Kind::Chat, Some(event.player), &event.raw_text, event.cancelled);
        Flow::Continue
    }

    fn player_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerCommandEvent<'_>,
    ) -> Flow {
        alert(host, bridge::alerts::Kind::PlayerCommand, Some(event.player), event.command, event.cancelled);
        let mut words = event.command.splitn(2, ' ');
        let is_discord = words.next().is_some_and(|root| {
            root.eq_ignore_ascii_case("discord") || root.eq_ignore_ascii_case("discordsrv")
        });
        if !is_discord {
            return Flow::Continue;
        }
        let args = words.next().unwrap_or("");
        let lines = discord_answer(host, Some((event.player, &*event.name)), args);
        event.cancelled = true;
        event.replies.extend(lines);
        Flow::Cancel
    }

    /// `/discord ...` from the console, RCON or a command block (DiscordSRV's commands take any
    /// `CommandSender`), and from a player through `execute`. A player's typed line is answered
    /// in `player_command`.
    fn run_mod_command(&self, host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
        let root = event.command.literals.first().map(String::as_str);
        if !matches!(root, Some("discord" | "discordsrv")) {
            return Flow::Continue;
        }
        // The words after the root: the literals, then the arguments (every usage has them in that order).
        let mut words: Vec<String> = event.command.literals[1..].to_vec();
        for argument in &event.command.arguments {
            if let CommandArgumentValue::String(text) = &argument.value {
                words.push(text.clone());
            }
        }
        let sender = match &event.source {
            CommandSource::Player { player, .. } => {
                Some((player.uuid(), player.name(&*host).unwrap_or_default()))
            }
            _ => None,
        };
        let lines = discord_answer(host, sender.as_ref().map(|(u, n)| (*u, n.as_str())), &words.join(" "));
        for line in lines {
            let _ = host.commands().feedback(line);
        }
        Flow::Cancel
    }

    fn permission_gather_nodes(&self, event: &mut PermissionGatherNodesEvent<'_>) {
        for (name, level) in NODES {
            let _ = event.nodes.add(node(name), node_default(level));
        }
        // plugin.yml's parent nodes: `discordsrv.player` (default true) and `discordsrv.admin` (op).
        let kids = |names: &[&str]| names.iter().map(|n| node(n)).collect::<Vec<_>>();
        let _ = event.nodes.add_with_children(
            node("discordsrv.player"),
            node_default(0),
            &kids(&[
                "discordsrv.chat",
                "discordsrv.help",
                "discordsrv.link",
                "discordsrv.linked",
                "discordsrv.discord",
                "discordsrv.nicknamesync",
            ]),
        );
        let _ = event.nodes.add_with_children(
            node("discordsrv.admin"),
            node_default(2),
            &kids(&[
                "discordsrv.player",
                "discordsrv.updatenotification",
                "discordsrv.bcast",
                "discordsrv.reload",
                "discordsrv.debug",
                "discordsrv.link.others",
                "discordsrv.linked.others",
                "discordsrv.unlink",
                "discordsrv.unlink.others",
                "discordsrv.resync",
                "discordsrv.groupsyncwithcommands",
                "discordsrv.language",
            ]),
        );
    }

    fn register_node_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterNodeCommandsEvent<'_>) {
        // Registered even when the bridge is off, so the commands answer with the line above.
        // Each usage names its own DiscordSRV node, as in the original's `plugin.yml` (the root
        // `discordsrv.discord` is everyone's). A usage keeps its level as the fallback when no
        // permissions mod answers. The `<x>.others` forms are checked again inside the bridge.
        // NOTE: `link <player>` takes a word, not a player argument, so an offline player can be
        // linked as DiscordSRV allows.
        const USAGES: [(&str, u8, &str); 20] = [
            ("", 0, "discordsrv.discord"),
            (" help [command:word]", 0, "discordsrv.help"),
            (" link", 0, "discordsrv.link"),
            (" link <player:word> <discord:text>", 2, "discordsrv.link.others"),
            (" linked", 0, "discordsrv.linked"),
            (" linked <target:text>", 2, "discordsrv.linked.others"),
            (" unlink", 2, "discordsrv.unlink"),
            (" unlink <target:text>", 2, "discordsrv.unlink"),
            (" clearlinked", 2, "discordsrv.unlink"),
            (" clearlinked <target:text>", 2, "discordsrv.unlink"),
            (" broadcast <message:text>", 2, "discordsrv.bcast"),
            (" bcast <message:text>", 2, "discordsrv.bcast"),
            (" resync", 2, "discordsrv.resync"),
            (" reload", 2, "discordsrv.reload"),
            (" language [language:word]", 2, "discordsrv.language"),
            (" setlanguage [language:word]", 2, "discordsrv.language"),
            (" lang [language:word]", 2, "discordsrv.language"),
            (" setlang [language:word]", 2, "discordsrv.language"),
            (" debugger [action:word] [categories:text]", 2, "discordsrv.debug"),
            // Any other word (`?`, or a word DiscordSRV answers with "That command doesn't exist!"):
            // the registered words are literals, so `?` needs a word argument to get through the tree.
            (" <sub:word> [args:text]", 0, "discordsrv.discord"),
        ];
        for root in ["discord", "discordsrv"] {
            for (args, level, node_name) in USAGES {
                event.commands.push(Command::op(&format!("{root}{args}"), level).with_node(&node(node_name)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_death_keys_have_vanilla_english_lines() {
        assert_eq!(death_line(Some("death.attack.lava"), "Tester"), "Tester tried to swim in lava");
        assert_eq!(death_line(Some("death.attack.fall"), "Tester"), "Tester hit the ground too hard");
        assert_eq!(death_line(Some("death.attack.freeze"), "Tester"), "Tester froze to death");
    }

    #[test]
    fn unknown_or_missing_death_keys_fall_back_to_the_generic_line() {
        assert_eq!(death_line(Some("death.attack.player"), "Tester"), "Tester died");
        assert_eq!(death_line(None, "Tester"), "Tester died");
    }

    #[test]
    fn other_death_keys_come_from_the_lang_table() {
        assert_eq!(death_line(Some("death.attack.cramming"), "Tester"), "Tester was squished too much");
        // A killer line needs its arguments: the generic one is the fallback.
        assert_eq!(death_line(Some("death.attack.mob"), "Tester"), "Tester died");
    }

    #[test]
    fn a_killer_and_a_weapon_are_named_in_the_death_line() {
        let by_mob = Text::translate(
            "death.attack.mob",
            vec![Text::literal("Tester"), Text::translate("entity.minecraft.zombie", vec![])],
        );
        assert_eq!(plain_text(&by_mob), "Tester was slain by Zombie");
        let by_item = Text::translate(
            "death.attack.player.item",
            vec![Text::literal("Tester"), Text::literal("Other"), Text::literal("[Diamond Sword]")],
        );
        assert_eq!(plain_text(&by_item), "Tester was slain by Other using [Diamond Sword]");
    }

    #[test]
    fn translatable_titles_are_flattened() {
        let t = Text::translate("advancements.story.mine_stone.title", vec![]);
        assert_eq!(plain_text(&t), "advancements.story.mine_stone.title");
        assert_eq!(plain_text(&Text::literal("Custom")), "Custom");
        assert_eq!(template("entity.minecraft.zombie_villager").as_deref(), Some("Zombie Villager"));
    }

    #[test]
    fn join_and_leave_lines_keep_their_text() {
        assert_eq!(vanilla_line("multiplayer.player.joined", "Tester").as_deref(), Some("Tester joined the game"));
        assert_eq!(vanilla_line("multiplayer.player.left", "Tester").as_deref(), Some("Tester left the game"));
        assert_eq!(vanilla_line("no.such.key", "Tester"), None);
    }
}
