//! `/discord` (alias `/discordsrv`) and its sub-commands, after DiscordSRV's `CommandManager`
//! and the `commands/*.java` classes: the sub-commands and their aliases, who may run them (the
//! `discordsrv.*` nodes of `plugin.yml` and their defaults), the argument parsing, the replies.
//! Every reply is a list of `&`-coded lines; the glue turns them into chat lines.
use serde_json::json;

use crate::chat;
use crate::format::{fill, translate_ampersand, truncate};
use crate::relay::Job;
use crate::server::{ServerInfo, parse_uuid, uuid_string};
use crate::{Shared, State};

/// The operator level of the `discordsrv.*` nodes that default to `op`.
pub const OP_LEVEL: u8 = 2;

const LINKING_ERROR: &str = "&cCurrently unable to link accounts due to an internal error. Contact your server administration team.";

/// One `@Command` of DiscordSRV.
struct Command {
    names: &'static [&'static str],
    help: &'static str,
    permission: &'static str,
    /// `default: op` in plugin.yml (else everyone).
    op: bool,
    example: &'static str,
}

/// In the order of `CommandManager`'s class list (the help lists them so).
static COMMANDS: [Command; 9] = [
    Command {
        names: &["broadcast", "bcast"],
        help: "Broadcasts a message to the main text channel on Discord",
        permission: "discordsrv.bcast",
        op: true,
        example: "broadcast <#channel?> Hello from the server!",
    },
    Command {
        names: &["debugger"],
        help: "A toggleable timings-like command to dump debug information to bin.scarsz.me",
        permission: "discordsrv.debug",
        op: true,
        example: "",
    },
    Command {
        names: &["?", "help"],
        help: "Shows command help for DiscordSRV's commands",
        permission: "discordsrv.help",
        op: false,
        example: "help [command]",
    },
    Command {
        names: &["language", "setlanguage", "lang", "setlang"],
        help: "Changes the language of DiscordSRV to whatever is specified.",
        permission: "discordsrv.language",
        op: true,
        example: "language japanese",
    },
    Command {
        names: &["link"],
        help: "Generates a code to link your Minecraft account to your Discord account",
        permission: "discordsrv.link",
        op: false,
        example: "",
    },
    Command {
        names: &["linked"],
        help: "Checks what Discord user your (or someone else's) MC account is linked to",
        permission: "discordsrv.linked",
        op: false,
        example: "",
    },
    Command {
        names: &["reload"],
        help: "Reloads the config of DiscordSRV",
        permission: "discordsrv.reload",
        op: true,
        example: "",
    },
    Command {
        names: &["resync"],
        help: "Resynchronizes all groups & roles",
        permission: "discordsrv.resync",
        op: true,
        example: "",
    },
    Command {
        names: &["unlink", "clearlinked"],
        help: "Unlinks your Minecraft account from your Discord account",
        permission: "discordsrv.unlink",
        op: true,
        example: "",
    },
];

/// Colour codes `CommandHelp` may pick for its title and command names (not black, dark blue,
/// gray, dark gray, white, or a format code).
const HELP_COLOURS: [char; 11] = ['2', '3', '4', '5', '6', '9', 'a', 'b', 'c', 'd', 'e'];

fn find(word: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.names.iter().any(|n| *n == word))
}

fn random_below(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u8(1);
    (hasher.finish() % n as u64) as usize
}

fn is_discord_id(text: &str) -> bool {
    (17..=20).contains(&text.len()) && text.chars().all(|c| c.is_ascii_digit())
}

/// The lines of `/discord help` (`args` empty) or `/discord help <command...>`.
fn help(has: &dyn Fn(&str, u8) -> bool, args: &[&str], unknown: &str) -> Vec<String> {
    let title = HELP_COLOURS[random_below(HELP_COLOURS.len())];
    let mut colour = title;
    while colour == title {
        colour = HELP_COLOURS[random_below(HELP_COLOURS.len())];
    }
    let chosen: Vec<&Command> = if args.is_empty() {
        COMMANDS.iter().collect()
    } else {
        let found: Vec<&Command> = args.iter().filter_map(|a| find(a)).collect();
        if found.is_empty() {
            return vec![unknown.to_string()];
        }
        found
    };
    let (bar, pad) = if args.is_empty() { ("================", "    ") } else { ("===================", "   ") };
    let mut out = vec![format!("&8{bar}[ &{title}DiscordSRV&8 ]{bar}")];
    for c in chosen {
        if !has(c.permission, if c.op { OP_LEVEL } else { 0 }) {
            continue;
        }
        out.push(format!("&7- &{colour}/discord {}", c.names.join("/")));
        out.push(format!("{pad}&o{}", c.help));
        if !c.example.is_empty() {
            out.push(format!("{pad}&7&oex. /discord {}", c.example));
        }
    }
    out
}

fn interpret(kind: &str) -> String {
    format!("&bInterpreted target as &f{kind}")
}

/// `- Player: name (uuid)`, `<Unknown>` without a name.
fn player_line(player: Option<(u128, Option<String>)>) -> String {
    let text = match player {
        Some((uuid, Some(name))) => format!("{name} ({})", uuid_string(uuid)),
        _ => "<Unknown>".to_string(),
    };
    format!("&f-&b Player: &f{text}")
}

/// `- Discord: name (#id) (tag) id`; DiscordSRV prints `null` for a missing ID.
fn discord_line(st: &State, id: Option<&str>) -> String {
    let user = id.and_then(|i| st.cache.members.get(i));
    let pretty = match (user, id) {
        (Some(m), Some(i)) => format!("{} (#{i})", m.effective_name()),
        _ => "<Unknown>".to_string(),
    };
    let info = user.map(|m| format!(" ({}#0000)", m.username)).unwrap_or_default();
    format!("&f-&b Discord: &f{pretty}{info} {}", id.unwrap_or("null"))
}

fn more_results(extra: usize) -> String {
    format!("&b+&f{extra}&b more result{}...", if extra > 1 { "s" } else { "" })
}

/// A player's name when the server knows it (online, or in `usercache.json`).
pub(crate) fn known_name(st: &State, server: &ServerInfo, uuid: u128) -> Option<String> {
    st.players
        .iter()
        .find(|p| p.uuid == uuid)
        .map(|p| p.name.clone())
        .or_else(|| server.cached_players().into_iter().find(|(u, _)| *u == uuid).map(|(_, n)| n))
}

fn player_of(st: &State, server: &ServerInfo, uuid: u128) -> Option<(u128, Option<String>)> {
    Some((uuid, known_name(st, server, uuid)))
}

/// A Minecraft player by name: on the server, else in `usercache.json`.
fn player_by_name(st: &State, server: &ServerInfo, name: &str) -> Option<(u128, String)> {
    st.players
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| (p.uuid, p.name.clone()))
        .or_else(|| server.cached_players().into_iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(u, n)| (u, n)))
}

/// The Discord users (IDs, sorted) whose username or nickname is `joined` (`name` or `name#tag`).
fn discord_matches(st: &State, joined: &str) -> Vec<String> {
    let (username, discriminator) = joined.split_once('#').unwrap_or((joined, ""));
    let mut ids: Vec<String> = st
        .cache
        .members
        .iter()
        .filter(|(_, m)| {
            m.username.eq_ignore_ascii_case(username) || m.nick.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(username))
        })
        .filter(|_| "0000".contains(discriminator))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

impl Shared {
    /// `/discord <args>` typed by a player. `has(node, default_level)` says whether the player
    /// has a permission node (`default_level` 0 = everyone, 2 = operators). The lines are
    /// `§`-coded; an empty list means the command is done without a reply.
    pub(crate) fn discord_command(&self, uuid: u128, name: &str, args: &str, has: &dyn Fn(&str, u8) -> bool) -> Vec<String> {
        let c = &self.config.chat;
        let words: Vec<&str> = args.split(' ').filter(|w| !w.is_empty()).collect();
        let Some((sub, rest)) = words.split_first() else {
            let text = c.discord_command_format.replace("{INVITE}", &c.discord_invite_link);
            return text.split('\n').map(translate_ampersand).collect();
        };
        let Some(command) = find(&sub.to_ascii_lowercase()) else {
            return vec![translate_ampersand(&c.unknown_command_message)];
        };
        if !has(command.permission, if command.op { OP_LEVEL } else { 0 }) {
            return vec![translate_ampersand(&c.no_permission_message)];
        }
        let lines = match command.names[0] {
            "broadcast" => self.cmd_broadcast(rest),
            "debugger" => vec!["&cThe debugger is not part of this port.".to_string()],
            "?" => help(has, rest, &c.unknown_command_message),
            "language" => cmd_language(rest),
            "link" => self.cmd_link(uuid, name, rest, has),
            "linked" => self.cmd_linked(uuid, rest, has),
            "reload" => vec!["&cDiscordSRV cannot reload its config yet: restart the server to apply changes.".to_string()],
            "resync" => vec![
                "&cGroup synchonization requires valid GroupRoleSynchronizationGroupsAndRolesToSync entries in synchronization.yml"
                    .to_string(),
            ],
            _ => self.cmd_unlink(uuid, name, rest, has),
        };
        lines.iter().map(|l| translate_ampersand(l)).collect()
    }

    /// `/discord link` (a code) or `/discord link <player> <discord user>` (staff).
    fn cmd_link(&self, uuid: u128, name: &str, args: &[&str], has: &dyn Fn(&str, u8) -> bool) -> Vec<String> {
        let c = &self.config.chat;
        let mut st = self.lock_state();
        if args.len() >= 2 {
            if !has("discordsrv.link.others", OP_LEVEL) {
                return vec![c.no_permission_message.clone()];
            }
            let target = if let Some(u) = parse_uuid(args[0]) {
                Some((u, known_name(&st, &self.server, u)))
            } else {
                player_by_name(&st, &self.server, args[0]).map(|(u, n)| (u, Some(n)))
            };
            let Some((target, target_name)) = target else {
                return vec!["&cMinecraft player could not be found".to_string()];
            };
            let who = args[1..].join(" ");
            let Some(discord) = find_user(&st, &who) else {
                return vec!["&cDiscord user could not be found".to_string()];
            };
            if let Err(e) = st.accounts.link(target, &discord) {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't save the linked accounts: {e}");
                return vec![LINKING_ERROR.to_string()];
            }
            let tag = st.cache.members.get(&discord).map_or_else(|| discord.clone(), |m| m.username.clone());
            let player = target_name.unwrap_or_else(|| uuid_string(target));
            self.push_job(Job::Linked { discord_name: tag.clone(), discord, uuid: target, name: player.clone() });
            return vec![format!("&aLinked together &6{player}&a and &6{tag}")];
        }
        // The player's earlier codes stop working, so no one can collect several rewards.
        st.accounts.drop_codes(uuid);
        if st.accounts.discord_of(uuid).is_some() {
            return vec![c.minecraft_account_already_linked.clone()];
        }
        let code = st.accounts.new_code(uuid, name);
        let bot = st.cache.bot_name.clone();
        vec![fill(&c.code_generated, &[("code", &code), ("botname", &bot)])]
    }

    /// `/discord linked` or `/discord linked <player or Discord user>`.
    fn cmd_linked(&self, uuid: u128, args: &[&str], has: &dyn Fn(&str, u8) -> bool) -> Vec<String> {
        let c = &self.config.chat;
        let st = self.lock_state();
        if args.is_empty() {
            return vec![match st.accounts.discord_of(uuid) {
                Some(id) => {
                    let who = st.cache.members.get(id).map_or_else(|| format!("Discord ID {id}"), |m| m.effective_name().to_string());
                    fill(&c.linked_command_success, &[("name", &who)])
                }
                None => c.minecraft_no_linked_account.clone(),
            }];
        }
        if !has("discordsrv.linked.others", OP_LEVEL) {
            return vec![c.no_permission_message.clone()];
        }
        let target = args[0];
        let joined = args.join(" ");
        let mut out = Vec::new();
        // Java's `a && b || c`: a 36 character target counts as a UUID whatever follows it.
        if (args.len() == 1 && target.len() == 32) || target.len() == 36 {
            out.push(interpret("UUID"));
            if let Some(u) = parse_uuid(target) {
                out.push(player_line(player_of(&st, &self.server, u)));
                out.push(discord_line(&st, st.accounts.discord_of(u)));
            }
            return out;
        }
        if (args.len() == 1 && st.cache.members.contains_key(target)) || is_discord_id(target) {
            out.push(interpret("Discord ID"));
            let linked = st.accounts.uuid_of(target).and_then(|u| player_of(&st, &self.server, u));
            out.push(player_line(linked));
            out.push(discord_line(&st, Some(target)));
            return out;
        }
        if joined.contains('#') || (2..=37).contains(&joined.chars().count()) {
            let matches = discord_matches(&st, &joined);
            if !matches.is_empty() {
                out.push(interpret("Discord name"));
                for id in matches.iter().take(5) {
                    let linked = st.accounts.uuid_of(id).and_then(|u| player_of(&st, &self.server, u));
                    out.push(player_line(linked));
                    out.push(discord_line(&st, Some(id)));
                }
                if matches.len() > 5 {
                    out.push(more_results(matches.len() - 5));
                }
                return out;
            }
        }
        if args.len() == 1 && (3..=16).contains(&target.len()) {
            if let Some((u, n)) = player_by_name(&st, &self.server, target) {
                out.push(interpret("Minecraft player"));
                out.push(player_line(Some((u, Some(n)))));
                out.push(discord_line(&st, st.accounts.discord_of(u)));
                return out;
            }
        }
        vec![fill(&c.minecraft_nobody_found, &[("target", &joined)])]
    }

    /// `/discord unlink` (own account) or `/discord unlink <player or Discord user>`; also
    /// `clearlinked`.
    fn cmd_unlink(&self, uuid: u128, name: &str, args: &[&str], has: &dyn Fn(&str, u8) -> bool) -> Vec<String> {
        let c = &self.config.chat;
        let mut st = self.lock_state();
        if args.is_empty() {
            let Some(id) = st.accounts.discord_of(uuid).map(str::to_string) else {
                return vec![c.minecraft_no_linked_account.clone()];
            };
            let who = st.cache.members.get(&id).map_or_else(|| format!("Discord ID {id}"), |m| m.effective_name().to_string());
            self.unlink_player(&mut st, uuid, Some(name.to_string()));
            return vec![fill(&c.unlink_command_success, &[("name", &who)])];
        }
        // DiscordSRV asks for `discordsrv.linked.others` here, not an unlink node.
        if !has("discordsrv.linked.others", OP_LEVEL) {
            return vec![c.no_permission_message.clone()];
        }
        let target = args[0];
        let joined = args.join(" ");
        let mut out = Vec::new();
        let unlinked = "&f- &b Unlinked ✓".to_string();
        if target.len() == 32 || (target.len() == 36 && args.len() == 1) {
            out.push(interpret("UUID"));
            if let Some(u) = parse_uuid(target) {
                out.push(player_line(player_of(&st, &self.server, u)));
                let discord = st.accounts.discord_of(u).map(str::to_string);
                out.push(discord_line(&st, discord.as_deref()));
                if discord.is_some() {
                    let known = known_name(&st, &self.server, u);
                    self.unlink_player(&mut st, u, known);
                    out.push(unlinked.clone());
                }
            }
            return out;
        }
        if (args.len() == 1 && st.cache.members.contains_key(target)) || is_discord_id(target) {
            out.push(interpret("Discord ID"));
            let linked = st.accounts.uuid_of(target);
            out.push(player_line(linked.and_then(|u| player_of(&st, &self.server, u))));
            out.push(discord_line(&st, Some(target)));
            if let Some(u) = linked {
                let known = known_name(&st, &self.server, u);
                self.unlink_player(&mut st, u, known);
                out.push(unlinked.clone());
            }
            return out;
        }
        if args.len() == 1 && (3..=16).contains(&target.len()) {
            if let Some((u, n)) = player_by_name(&st, &self.server, target) {
                out.push(interpret("Minecraft player"));
                out.push(player_line(Some((u, Some(n.clone())))));
                let discord = st.accounts.discord_of(u).map(str::to_string);
                out.push(discord_line(&st, discord.as_deref()));
                self.unlink_player(&mut st, u, Some(n));
                out.push(unlinked.clone());
                return out;
            }
        }
        if joined.contains('#') || (2..=37).contains(&joined.chars().count()) {
            let matches = discord_matches(&st, &joined);
            if !matches.is_empty() {
                out.push(interpret("Discord name"));
                if matches.len() == 1 {
                    let id = &matches[0];
                    out.push(discord_line(&st, Some(id)));
                    match st.accounts.uuid_of(id) {
                        Some(u) => {
                            out.push(player_line(player_of(&st, &self.server, u)));
                            let known = known_name(&st, &self.server, u);
                            self.unlink_player(&mut st, u, known);
                            out.push(unlinked.clone());
                        }
                        None => out.push(player_line(None)),
                    }
                } else {
                    for id in matches.iter().take(5) {
                        let linked = st.accounts.uuid_of(id).and_then(|u| player_of(&st, &self.server, u));
                        out.push(player_line(linked));
                        out.push(discord_line(&st, Some(id)));
                    }
                    if matches.len() > 5 {
                        out.push(more_results(matches.len() - 5));
                    }
                    out.push("&bBe more specific.".to_string());
                }
                return out;
            }
        }
        vec![fill(&c.minecraft_nobody_found, &[("target", &joined)])]
    }

    /// Unlinks a player and queues the role and console commands that go with it.
    fn unlink_player(&self, st: &mut State, uuid: u128, name: Option<String>) {
        match st.accounts.unlink(uuid) {
            Ok(Some(discord)) => {
                self.push_job(Job::Unlinked { discord, uuid, name: name.unwrap_or_else(|| uuid_string(uuid)) });
            }
            Ok(None) => {}
            Err(e) => log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't save the linked accounts: {e}"),
        }
    }

    /// `/discord broadcast [#channel] <message>`: nothing is said back when it works.
    fn cmd_broadcast(&self, args: &[&str]) -> Vec<String> {
        let c = &self.config.chat;
        let st = self.lock_state();
        let (target, text_args): (Option<String>, &[&str]) = match args.first().filter(|a| a.starts_with('#')) {
            Some(first) => {
                let raw = first.replace('#', "");
                let by_id = (!raw.is_empty() && raw.chars().all(|ch| ch.is_ascii_digit()) && st.cache.channels.contains_key(&raw))
                    .then(|| raw.clone());
                (by_id.or_else(|| st.cache.find_channel(&raw).map(str::to_string)), &args[1..])
            }
            None => (c.channel_for("broadcasts").map(str::to_string), args),
        };
        if text_args.is_empty() {
            return vec!["&cNo text given to broadcast".to_string()];
        }
        let mut text = text_args.join(" ").replace("\\n", "\n");
        if c.discord_chat_channel_translate_mentions {
            text = chat::translate_mentions(c, &st.cache, &text);
        }
        if let Some(channel) = target
            && !text.trim().is_empty()
        {
            let content = truncate(&text, crate::console::MAX_MESSAGE);
            self.push_job(Job::Post { channel, body: json!({ "content": content, "allowed_mentions": chat::allowed_mentions(c) }), expire: None });
        }
        Vec::new()
    }
}

/// A Discord user for `/discord link`: an ID, or the name of one the bot has seen.
fn find_user(st: &State, who: &str) -> Option<String> {
    if is_discord_id(who) || st.cache.members.contains_key(who) {
        return Some(who.to_string());
    }
    let name = who.rsplit_once('#').map_or(who, |(n, _)| n);
    st.cache.find_member(name).map(str::to_string)
}

/// The languages DiscordSRV ships files for; this port's messages are English only.
const LANGUAGES: [(&str, &str); 14] = [
    ("da", "Danish"),
    ("de", "German"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fr", "French"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("nb", "Norwegian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("zh", "Chinese"),
];

/// `/discord language [name or code] [-confirm]`.
fn cmd_language(args: &[&str]) -> Vec<String> {
    let target = args.iter().find_map(|a| LANGUAGES.iter().find(|(code, name)| code.eq_ignore_ascii_case(a) || name.eq_ignore_ascii_case(a)));
    let Some((code, name)) = target else {
        return vec!["&3DiscordSRV is currently in English. Change it by giving a language as an argument.".to_string()];
    };
    if *code != "en" {
        return vec![format!("&3DiscordSRV does not have a translation for {name}. Supported languages are as follows: English.")];
    }
    if !args.iter().any(|a| a.eq_ignore_ascii_case("-confirm")) {
        return vec![
            "&3This will reset your DiscordSRV configuration files to be in &fEnglish&3. Your old config files will be renamed to have &fEnglish.&3 on the beginning of the file name. &a[Confirm by running the command again, adding \" -confirm\" to the end]"
                .to_string(),
        ];
    }
    // Guess: this port's config is one TOML file in English; there is nothing to reset.
    vec!["&3DiscordSRV language successfully changed to English.".to_string()]
}
