//! The original DiscordSRV 1.30.5 files, read as they are: `<config>/discordsrv/config.yml` with its
//! siblings `messages.yml`, `linking.yml`, `alerts.yml`, `synchronization.yml` and `voice.yml` (the copy
//! of `plugins/DiscordSRV/`). Their keys are [`Config`]'s field names in PascalCase (the chat keys are
//! flattened in), so the YAML becomes the same JSON shape serde reads from the TOML file. A few maps are
//! ordered pairs in the port (`Channels`, the two filter lists), ID-like scalars are strings even when
//! they are not quoted, and a `null` inside a list or pair becomes an empty string.
//!
//! Nothing here prints a value. Errors name the file and the line only. The bot token is taken out of the
//! tree before [`Config`] is built and returned on its own, so it never sits in a `Debug` print.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::config::Config;
use crate::yaml::{self, Node};

/// The files of the DiscordSRV folder, merged in this order. `config.yml` and `messages.yml` are required.
pub const FILES: [&str; 6] = ["config.yml", "messages.yml", "linking.yml", "alerts.yml", "synchronization.yml", "voice.yml"];

const REQUIRED: [&str; 2] = ["config.yml", "messages.yml"];

const TOKEN_KEY: &str = "BotToken";

/// Maps the port reads as ordered `(key, value)` pairs (`Vec<(String, String)>` in [`Config`]).
const PAIR_MAPS: [&str; 4] = [
    "Channels",
    "DiscordConsoleChannelFilters",
    "DiscordChatChannelGameFilters",
    "DiscordChatChannelDiscordFilters",
];

/// Keys outside [`Config`] that are read only to say which switched-on features the port does not do.
const FLAG_KEYS: [&str; 7] = [
    "Require linked account to play",
    "NicknameSynchronizationEnabled",
    "GroupRoleSynchronizationGroupsAndRolesToSync",
    "BanSynchronizationDiscordToMinecraft",
    "BanSynchronizationMinecraftToDiscord",
    "Voice enabled",
    "Alerts",
];

/// What [`load`] gives: the settings, the bot token (kept apart), and the switched-on features that
/// the original has but the port does not run (names only, for a warning).
pub struct Original {
    pub config: Config,
    pub token: String,
    pub unported: Vec<&'static str>,
}

/// Loads the DiscordSRV folder `dir`. `Ok(None)` when there is no `config.yml` (the caller falls back to
/// the TOML file). Errors (a missing required file, a parse error, a value of the wrong type) refuse the
/// whole load: a half-read config must not post anything.
pub fn load(dir: &Path) -> Result<Option<Original>, String> {
    if !dir.join("config.yml").exists() {
        return Ok(None);
    }
    let mut root = Map::new();
    for name in FILES {
        let path = dir.join(name);
        if !path.exists() {
            if REQUIRED.contains(&name) {
                return Err(format!("{name}: missing"));
            }
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|_| format!("{name}: unreadable"))?;
        let node = yaml::parse(&text).map_err(|e| format!("{name}: {e}"))?;
        for entry in node.entries() {
            root.insert(entry.key.clone(), to_json(&entry.value, &entry.key));
        }
    }
    let unported = unported(&root);
    let token = match root.remove(TOKEN_KEY) {
        Some(Value::String(t)) => t.trim().to_string(),
        _ => String::new(),
    };
    let mut config: Config = serde_json::from_value(Value::Object(root)).map_err(|e| format!("config: {e}"))?;
    // DiscordSRV's placeholder lists: empty strings and channel updaters with the zero placeholder ID do nothing.
    let chat = &mut config.chat;
    chat.minecraft_discord_account_linked_console_commands.retain(|c| !c.trim().is_empty());
    chat.minecraft_discord_account_unlinked_console_commands.retain(|c| !c.trim().is_empty());
    chat.channel_updater.retain(|u| is_real_id(&u.channel_id));
    chat.channels.retain(|(_, id)| is_real_id(id));
    Ok(Some(Original { config, token, unported }))
}

/// An ID that is not DiscordSRV's zero placeholder.
fn is_real_id(id: &str) -> bool {
    id.trim().chars().any(|c| c.is_ascii_digit() && c != '0')
}

/// The switched-on features of the original that the port does not run.
fn unported(root: &Map<String, Value>) -> Vec<&'static str> {
    let on = |k: &str| root.get(k).and_then(Value::as_bool) == Some(true);
    let mut out = Vec::new();
    if on("Require linked account to play") {
        out.push("linking.yml: require linked account to play");
    }
    if on("NicknameSynchronizationEnabled") {
        out.push("synchronization.yml: nickname synchronisation");
    }
    if on("BanSynchronizationDiscordToMinecraft") || on("BanSynchronizationMinecraftToDiscord") {
        out.push("synchronization.yml: ban synchronisation");
    }
    if root
        .get("GroupRoleSynchronizationGroupsAndRolesToSync")
        .and_then(Value::as_object)
        .is_some_and(|m| m.values().any(|id| id.as_str().is_some_and(is_real_id)))
    {
        out.push("synchronization.yml: group and role synchronisation");
    }
    if on("Voice enabled") {
        out.push("voice.yml: voice");
    }
    if root.get("Alerts").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
        out.push("alerts.yml: alerts");
    }
    out
}

/// A key is an ID when its name says so: the IDs of channels, roles and users, and the `Channels` values.
fn id_key(key: &str) -> bool {
    key.ends_with("Id") || key.ends_with("Ids") || key == "Channels"
}

/// A YAML node as JSON. `key` is the name of the entry holding it (what the scalar means).
fn to_json(node: &Node, key: &str) -> Value {
    match node {
        Node::Scalar { value, .. } => scalar(node, value, key),
        Node::Seq { items, .. } => Value::Array(items.iter().map(|n| to_json(n, key)).map(non_null).collect()),
        Node::Map { entries, .. } if PAIR_MAPS.contains(&key) => Value::Array(
            entries
                .iter()
                .map(|e| Value::Array(vec![Value::String(e.key.clone()), non_null(to_json(&e.value, key))]))
                .collect(),
        ),
        Node::Map { entries, .. } => Value::Object(
            entries
                .iter()
                .map(|e| (e.key.clone(), to_json(&e.value, &e.key)))
                .filter(|(_, v)| !v.is_null())
                .collect(),
        ),
    }
}

/// A `null` where a string belongs (in a list or a pair) is the empty string.
fn non_null(v: Value) -> Value {
    if v.is_null() { Value::String(String::new()) } else { v }
}

fn scalar(node: &Node, text: &str, key: &str) -> Value {
    match node.tag() {
        "bool" => node.as_bool().map(Value::Bool).unwrap_or(Value::Null),
        "null" => Value::Null,
        "int" | "float" if !id_key(key) => {
            let number = if node.tag() == "int" {
                node.as_i64().map(serde_json::Number::from)
            } else {
                node.as_f64().and_then(serde_json::Number::from_f64)
            };
            number.map(Value::Number).unwrap_or_else(|| Value::String(text.to_string()))
        }
        _ => Value::String(text.to_string()),
    }
}

/// The names of every top-level key `Config` reads (the PascalCase names of its fields, the token aside).
pub fn known_keys() -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = match serde_json::to_value(Config::default()) {
        Ok(Value::Object(m)) => m.keys().cloned().collect(),
        _ => BTreeSet::new(),
    };
    keys.insert(TOKEN_KEY.to_string());
    keys
}

/// What one file's top-level keys are used for: the names of the keys the port reads and of those it does not.
/// Names only, never a value.
pub struct Summary {
    pub file: String,
    pub read: Vec<String>,
    pub unread: Vec<String>,
    /// The parse error (line and message), when the file does not parse.
    pub error: Option<String>,
}

pub fn summarize(file: &str, text: &str) -> Summary {
    let known = known_keys();
    match yaml::parse(text) {
        Err(e) => Summary { file: file.to_string(), read: vec![], unread: vec![], error: Some(e.to_string()) },
        Ok(root) => {
            let (mut read, mut unread) = (Vec::new(), Vec::new());
            for key in root.keys() {
                if known.contains(key) || FLAG_KEYS.contains(&key) {
                    read.push(key.to_string());
                } else {
                    unread.push(key.to_string());
                }
            }
            Summary { file: file.to_string(), read, unread, error: None }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented values only: the same structure as the original, nothing from a real server.
    const CONFIG: &str = r#"ConfigVersion: 1.30.5
BotToken: "TEST-TOKEN-NOT-REAL"
DiscordConsoleChannelId: 111111111111111111
Channels: {"global": 222222222222222222, "deaths": "333333333333333333"}
DiscordConsoleChannelFilters: {"zeta": "", "alpha": "<x>", "\\d+": "<n>"}
DiscordConsoleChannelLevels: [info, warn, error]
DiscordConsoleChannelLogRefreshRateInSeconds: 7
DiscordGameStatus: ["playing Minecraft", "watching Things"]
DiscordOnlineStatus: ONLINE
StatusUpdateRateInMinutes: 3
ChannelUpdater:
  - ChannelId: 444444444444444444
    Format: "%playercount% online"
    ShutdownFormat: "Offline"
    UpdateInterval: 10
  - ChannelId: "000000000000000000"
    Format: "placeholder"
    ShutdownFormat: ""
    UpdateInterval: 10
MinecraftDiscordAccountLinkedConsoleCommands: ["", "say linked"]
ProxyHost: "example.invalid"
Experiment_JdbcUsername: "not-a-real-user"
DiscordChatChannelRoleAliases: {"Developer": "Dev"}
"#;

    const MESSAGES: &str = "DiscordCommandFormat: \"&bJoin {INVITE}\"\nDiscordConsoleChannelPadding: 4\n";

    fn folder(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hdsrv-original-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    #[test]
    fn no_config_yml_means_no_original() {
        let dir = folder("none", &[("config.toml", "x = 1")]);
        assert!(load(&dir).unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_original_keys_become_the_config() {
        let dir = folder("keys", &[("config.yml", CONFIG), ("messages.yml", MESSAGES)]);
        let o = load(&dir).unwrap().unwrap();
        let c = &o.config;
        assert_eq!(c.discord_console_channel_id, "111111111111111111");
        assert_eq!(c.discord_console_channel_log_refresh_rate_in_seconds, 7);
        assert_eq!(c.status_update_rate_in_minutes, 3);
        assert_eq!(c.discord_console_channel_padding, 4);
        assert_eq!(c.discord_game_status, vec!["playing Minecraft".to_string(), "watching Things".to_string()]);
        assert_eq!(c.chat.discord_chat_channel_role_aliases.get("Developer").map(String::as_str), Some("Dev"));
        assert_eq!(c.chat.discord_command_format, "&bJoin {INVITE}");
        assert_eq!(c.chat.minecraft_discord_account_linked_console_commands, vec!["say linked".to_string()]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn maps_that_order_matter_keep_their_order_and_ids_are_strings() {
        let dir = folder("pairs", &[("config.yml", CONFIG), ("messages.yml", MESSAGES)]);
        let c = load(&dir).unwrap().unwrap().config;
        assert_eq!(
            c.discord_console_channel_filters,
            vec![
                ("zeta".to_string(), String::new()),
                ("alpha".to_string(), "<x>".to_string()),
                ("\\d+".to_string(), "<n>".to_string()),
            ]
        );
        assert_eq!(
            c.chat.channels,
            vec![
                ("global".to_string(), "222222222222222222".to_string()),
                ("deaths".to_string(), "333333333333333333".to_string()),
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lists_of_maps_become_channel_updaters_and_the_zero_placeholder_is_dropped() {
        let dir = folder("updaters", &[("config.yml", CONFIG), ("messages.yml", MESSAGES)]);
        let c = load(&dir).unwrap().unwrap().config;
        assert_eq!(c.chat.channel_updater.len(), 1);
        assert_eq!(c.chat.channel_updater[0].channel_id, "444444444444444444");
        assert_eq!(c.chat.channel_updater[0].shutdown_format, "Offline");
        assert_eq!(c.chat.channel_updater[0].update_interval, 10);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_token_is_returned_apart_and_never_in_the_config_print() {
        let dir = folder("token", &[("config.yml", CONFIG), ("messages.yml", MESSAGES)]);
        let o = load(&dir).unwrap().unwrap();
        assert_eq!(o.token, "TEST-TOKEN-NOT-REAL");
        assert!(!format!("{:?}", o.config).contains("TEST-TOKEN-NOT-REAL"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn switched_on_features_the_port_lacks_are_named() {
        let voice = "Voice enabled: true\n";
        let dir = folder("unported", &[("config.yml", CONFIG), ("messages.yml", MESSAGES), ("voice.yml", voice)]);
        let o = load(&dir).unwrap().unwrap();
        assert_eq!(o.unported, vec!["voice.yml: voice"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_messages_file_refuses_the_load() {
        let dir = folder("nomsg", &[("config.yml", CONFIG)]);
        assert!(load(&dir).err().expect("an error").contains("messages.yml"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_parse_error_names_the_line_not_the_text() {
        let broken = "BotToken: \"TEST-TOKEN-NOT-REAL\nDiscordConsoleChannelId: 1\n";
        let dir = folder("broken", &[("config.yml", broken), ("messages.yml", MESSAGES)]);
        let e = load(&dir).err().expect("an error");
        assert!(e.starts_with("config.yml: line "), "{e}");
        assert!(!e.contains("TEST-TOKEN"), "{e}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_summary_names_unread_keys_and_no_values() {
        let s = summarize("config.yml", CONFIG);
        assert!(s.error.is_none());
        assert!(s.unread.contains(&"ProxyHost".to_string()));
        assert!(s.unread.contains(&"Experiment_JdbcUsername".to_string()));
        assert!(s.read.contains(&"DiscordConsoleChannelId".to_string()));
        assert!(s.read.contains(&"BotToken".to_string()));
        let text = format!("{:?}", s.read) + &format!("{:?}", s.unread);
        assert!(!text.contains("not-a-real-user") && !text.contains("example.invalid"));
    }
}
