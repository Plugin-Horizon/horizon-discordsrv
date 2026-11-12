//! `config.toml`: the keys DiscordSRV's `config.yml`, `messages.yml` and the account linking
//! part of `linking.yml` use, under the same names. The console channel keys are [`Config`]'s
//! own; the chat channels, notifications, linking and watchdog keys are [`Chat`]'s, flattened
//! in, and all of them off when missing (a console-only config stays console-only).
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The file written when none exists: the bridge stays off until a console channel is set.
pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Config {
    /// File holding the bot token (relative to the mod's config folder). Never the token itself.
    pub bot_token_file: String,
    /// The console channel's numerical ID; empty turns the bridge off.
    pub discord_console_channel_id: String,
    pub discord_game_status: Vec<String>,
    pub discord_online_status: String,
    pub discord_console_channel_log_refresh_rate_in_seconds: u64,
    pub discord_console_channel_usage_log: String,
    pub discord_console_channel_blacklist_acts_as_whitelist: bool,
    pub discord_console_channel_blacklisted_commands: Vec<String>,
    /// Ordered `[regex, replacement]` pairs; a line that ends up empty is not sent.
    pub discord_console_channel_filters: Vec<(String, String)>,
    pub discord_console_channel_levels: Vec<String>,
    pub discord_console_channel_use_code_blocks: bool,
    pub discord_console_channel_block_bots: bool,
    pub cancel_console_command_if_logging_failed: bool,
    pub discord_console_channel_timestamp_format: String,
    pub discord_console_channel_prefix: String,
    pub discord_console_channel_suffix: String,
    pub channel_topic_updater_rate_in_minutes: u64,
    pub channel_topic_updater_channel_topics_at_shutdown_enabled: bool,
    pub channel_topic_updater_console_channel_topic_format: String,
    pub channel_topic_updater_console_channel_topic_at_server_shutdown_format: String,
    /// Minutes between presence updates; each cycles to the next `discord_game_status` entry (0 = never).
    pub status_update_rate_in_minutes: u64,
    /// `DiscordConsoleChannelPadding` (messages.yml); read, not applied (see GAPS.md).
    pub discord_console_channel_padding: u64,
    /// Discord's REST base. Set by `ApiBaseUrl` in the TOML file, or by the environment variable
    /// `HORIZON_DISCORDSRV_API_BASE` (test only, see README).
    pub api_base_url: String,
    /// Chat channels, notifications, linking, watchdog (missing keys: off).
    #[serde(flatten)]
    pub chat: Chat,
}

/// The keys of the chat channel side. `Default` is everything off: a missing key never turns a
/// feature on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Chat {
    /// Ordered `[game channel, Discord channel ID]` pairs (`Channels` in config.yml). The first
    /// is the default; `global`, `status`, `awards`, `deaths`, `join`, `leave`, `watchdog` and
    /// `link` take their kind of message when present.
    pub channels: Vec<(String, String)>,
    pub discord_invite_link: String,
    /// Avatar URL pattern: `{username}`, `{uuid}`, `{uuid-nodashes}`, `{size}`, `{texture}`.
    /// Empty: [`DEFAULT_AVATAR_URL`].
    pub avatar_url: String,
    /// Java `SimpleDateFormat` pattern for `%date%` in chat messages and notifications.
    pub timestamp_format: String,
    /// Java `SimpleDateFormat` pattern for `%date%` in the usage log's file name (`DateFormat`;
    /// empty: `yyyy-MM-dd`).
    pub date_format: String,

    pub discord_chat_channel_discord_to_minecraft: bool,
    pub discord_chat_channel_minecraft_to_discord: bool,
    /// Longest Discord message shown in game, in characters; 0 = no limit.
    pub discord_chat_channel_truncate_length: usize,
    pub discord_chat_channel_translate_mentions: bool,
    /// `user`, `channel`, `emote`, `role`, `here`, `everyone`.
    pub discord_chat_channel_allowed_mentions: Vec<String>,
    /// `show`, `name` or `hide`.
    pub discord_chat_channel_emoji_behavior: String,
    /// `name` or `hide`.
    pub discord_chat_channel_emote_behavior: String,
    pub discord_chat_channel_prefix_required_to_process_message: String,
    pub discord_chat_channel_prefix_acts_as_blacklist: bool,
    pub discord_chat_channel_roles_allowed_to_use_color_codes_in_chat: Vec<String>,
    pub discord_chat_channel_broadcast_discord_messages_to_console: bool,
    pub discord_chat_channel_require_linked_account: bool,
    pub discord_chat_channel_block_bots: bool,
    pub discord_chat_channel_block_webhooks: bool,
    pub discord_chat_channel_blocked_ids: Vec<String>,
    pub discord_chat_channel_blocked_roles_as_whitelist: bool,
    pub discord_chat_channel_blocked_roles_ids: Vec<String>,
    pub discord_chat_channel_roles_selection_as_whitelist: bool,
    pub discord_chat_channel_roles_selection: Vec<String>,
    pub discord_chat_channel_role_aliases: HashMap<String, String>,
    pub discord_chat_channel_list_command_enabled: bool,
    pub discord_chat_channel_list_command_message: String,
    /// Seconds until the player list answer is deleted; 0 = kept.
    pub discord_chat_channel_list_command_expiration: u64,
    pub discord_chat_channel_list_command_expiration_delete_request: bool,
    /// Console commands typed in a chat channel (`!c <command>`).
    pub discord_chat_channel_console_command_enabled: bool,
    pub discord_chat_channel_console_command_notify_errors: bool,
    pub discord_chat_channel_console_command_prefix: String,
    pub discord_chat_channel_console_command_roles_allowed: Vec<String>,
    pub discord_chat_channel_console_command_whitelist: Vec<String>,
    pub discord_chat_channel_console_command_whitelist_bypass_roles: Vec<String>,
    pub discord_chat_channel_console_command_whitelist_acts_as_blacklist: bool,
    /// Seconds until the answer (and the request) is deleted; 0 = kept.
    pub discord_chat_channel_console_command_expiration: u64,
    pub discord_chat_channel_console_command_expiration_delete_request: bool,
    pub discord_chat_channel_console_command_notify_errors_format: String,
    /// Ordered `[regex, replacement]` pairs for Minecraft -> Discord chat.
    pub discord_chat_channel_game_filters: Vec<(String, String)>,
    /// Ordered `[regex, replacement]` pairs for Discord -> Minecraft chat.
    pub discord_chat_channel_discord_filters: Vec<(String, String)>,
    pub channel_updater: Vec<ChannelUpdater>,
    pub discord_canned_responses: HashMap<String, String>,

    pub minecraft_discord_account_linked_console_commands: Vec<String>,
    pub minecraft_discord_account_unlinked_console_commands: Vec<String>,
    pub minecraft_discord_account_linked_role_name_to_add_user_to: String,
    pub minecraft_discord_account_linked_allow_relink_by_sending_a_new_code: bool,
    #[serde(rename = "MinecraftDiscordAccountLinkedUsePM")]
    pub minecraft_discord_account_linked_use_pm: bool,
    /// Seconds until the answer in the `link` channel and the code message are deleted; 0 = kept.
    pub minecraft_discord_account_linked_message_delete_seconds: u64,
    /// Accounts file beside config.toml (DiscordSRV's `accounts.aof`).
    pub accounts_file: String,

    pub server_watchdog_enabled: bool,
    /// Seconds without a tick; at least 10.
    pub server_watchdog_timeout: u64,
    pub server_watchdog_message_count: u32,

    // messages.yml
    pub discord_to_minecraft_chat_message_format: String,
    pub discord_to_minecraft_chat_message_format_no_role: String,
    pub discord_to_minecraft_all_roles_separator: String,
    pub discord_to_minecraft_message_reply_format: String,
    pub minecraft_chat_to_discord_message_format: String,
    pub minecraft_chat_to_discord_message_format_no_primary_group: String,
    pub discord_chat_channel_list_command_format_online_players: String,
    pub discord_chat_channel_list_command_format_no_online_players: String,
    pub discord_chat_channel_list_command_player_format: String,
    pub discord_chat_channel_list_command_all_players_separator: String,
    pub minecraft_player_join_message: MessageTemplate,
    pub minecraft_player_first_join_message: MessageTemplate,
    pub minecraft_player_leave_message: MessageTemplate,
    pub minecraft_player_death_message: MessageTemplate,
    pub minecraft_player_achievement_message: MessageTemplate,
    pub channel_topic_updater_chat_channel_topic_format: String,
    pub channel_topic_updater_chat_channel_topic_at_server_shutdown_format: String,
    pub discord_command_format: String,
    pub unknown_command_message: String,
    pub discord_chat_channel_server_startup_message: String,
    pub discord_chat_channel_server_shutdown_message: String,
    pub server_watchdog_message: String,
    pub unknown_code: String,
    pub invalid_code: String,
    pub discord_account_linked: String,
    pub discord_account_already_linked: String,
    pub discord_linked_account_required: String,
    pub code_generated: String,
    pub minecraft_account_linked: String,
    pub minecraft_account_already_linked: String,
    pub linked_command_success: String,
    pub unlink_command_success: String,
    pub minecraft_no_linked_account: String,
    pub no_permission_message: String,
    pub minecraft_nobody_found: String,
    pub linking_error: String,
    pub click_to_copy_code: String,
    pub discord_linked_account_check_failed: String,
}

/// One `ChannelUpdater` entry: a channel whose name shows server information.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ChannelUpdater {
    pub channel_id: String,
    pub format: String,
    pub shutdown_format: String,
    /// Minutes; Discord allows a rename every 10 minutes at most, so less counts as 10.
    pub update_interval: u64,
}

/// A notification of messages.yml (`MinecraftPlayerJoinMessage` and its siblings). Webhook
/// delivery is not ported (see the README).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MessageTemplate {
    pub enabled: bool,
    pub content: String,
    pub embed: EmbedTemplate,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EmbedTemplate {
    pub enabled: bool,
    /// `#rrggbb` or a decimal RGB integer.
    pub color: String,
    pub author: Link3,
    pub thumbnail_url: String,
    pub title: TitleTemplate,
    pub description: String,
    /// `title;value;inline` or `blank`.
    pub fields: Vec<String>,
    pub image_url: String,
    pub footer: FooterTemplate,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Link3 {
    pub image_url: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TitleTemplate {
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FooterTemplate {
    pub text: String,
    pub icon_url: String,
}

/// `Timestamp`: `true` for the time of sending, or a fixed epoch second.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Timestamp {
    Now(bool),
    Epoch(i64),
}

impl Default for Timestamp {
    fn default() -> Self {
        Timestamp::Now(false)
    }
}

/// The avatar used when `AvatarUrl` is empty.
pub const DEFAULT_AVATAR_URL: &str = "https://crafatar.com/avatars/{uuid-nodashes}?size={size}&overlay";

impl Chat {
    /// The Discord channel for a kind of message: its own entry, else the first channel.
    pub fn channel_for(&self, kind: &str) -> Option<&str> {
        self.channels
            .iter()
            .find(|(game, _)| game.eq_ignore_ascii_case(kind))
            .or(self.channels.first())
            .map(|(_, id)| id.as_str())
            .filter(|id| !id.trim().is_empty())
    }

    /// The Discord channel of the game channel `kind` only (no fallback to the first).
    pub fn own_channel(&self, kind: &str) -> Option<&str> {
        self.channels
            .iter()
            .find(|(game, _)| game.eq_ignore_ascii_case(kind))
            .map(|(_, id)| id.as_str())
            .filter(|id| !id.trim().is_empty())
    }

    /// Whether a Discord channel is one of the chat channels.
    pub fn is_chat_channel(&self, id: &str) -> bool {
        self.channels.iter().any(|(_, c)| c == id)
    }

    pub fn has_channels(&self) -> bool {
        self.channels.iter().any(|(_, id)| !id.trim().is_empty())
    }

    pub fn mention_allowed(&self, kind: &str) -> bool {
        self.discord_chat_channel_allowed_mentions.iter().any(|m| m.eq_ignore_ascii_case(kind))
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bot_token_file: "token.txt".into(),
            discord_console_channel_id: String::new(),
            discord_game_status: vec!["playing Minecraft".into()],
            discord_online_status: "ONLINE".into(),
            discord_console_channel_log_refresh_rate_in_seconds: 5,
            discord_console_channel_usage_log: "Console-%date%.log".into(),
            discord_console_channel_blacklist_acts_as_whitelist: false,
            discord_console_channel_blacklisted_commands: Vec::new(),
            discord_console_channel_filters: Vec::new(),
            discord_console_channel_levels: vec!["info".into(), "warn".into(), "error".into()],
            discord_console_channel_use_code_blocks: true,
            discord_console_channel_block_bots: true,
            cancel_console_command_if_logging_failed: true,
            discord_console_channel_timestamp_format: "EEE HH:mm:ss".into(),
            discord_console_channel_prefix: "[{date} {level}{name}] ".into(),
            discord_console_channel_suffix: String::new(),
            channel_topic_updater_rate_in_minutes: 10,
            channel_topic_updater_channel_topics_at_shutdown_enabled: true,
            channel_topic_updater_console_channel_topic_format: String::new(),
            channel_topic_updater_console_channel_topic_at_server_shutdown_format: String::new(),
            status_update_rate_in_minutes: 2,
            discord_console_channel_padding: 0,
            api_base_url: "https://discord.com/api/v10".into(),
            chat: Chat::default(),
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Whether a command typed in the console channel may run: the list is a blacklist, or a
    /// whitelist when `DiscordConsoleChannelBlacklistActsAsWhitelist` is on. As in DiscordSRV the
    /// first word of the message, lower case and without a `namespace:` before it, is looked up
    /// in the list (also a listed phrase of several words matches a command starting with it).
    pub fn command_allowed(&self, command: &str) -> bool {
        let command = command.trim().trim_start_matches('/').to_lowercase();
        if command.is_empty() {
            return false;
        }
        let first = command.split(' ').next().unwrap_or_default();
        let requested = first.rsplit(':').next().unwrap_or(first);
        let listed = self.discord_console_channel_blacklisted_commands.iter().any(|p| {
            let p = p.trim().to_lowercase();
            !p.is_empty() && (requested == p || (p.contains(' ') && (command == p || command.starts_with(&format!("{p} ")))))
        });
        listed == self.discord_console_channel_blacklist_acts_as_whitelist
    }

    pub fn level_enabled(&self, level: &str) -> bool {
        self.discord_console_channel_levels.iter().any(|l| l.eq_ignore_ascii_case(level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_file_parses() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.discord_console_channel_id, "");
        assert_eq!(c.discord_console_channel_log_refresh_rate_in_seconds, 5);
        assert!(c.discord_console_channel_blacklist_acts_as_whitelist);
        assert!(c.discord_console_channel_blacklisted_commands.is_empty());
        assert_eq!(c.discord_console_channel_filters.len(), 4);
        assert_eq!(c.discord_console_channel_prefix, "[{date} {level}{name}] ");
        assert_eq!(c.channel_topic_updater_rate_in_minutes, 10);
        assert_eq!(c.discord_game_status, vec!["playing Minecraft".to_string()]);
    }

    #[test]
    fn the_example_chat_keys_parse() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap().chat;
        assert!(c.channels.is_empty());
        assert!(c.discord_chat_channel_discord_to_minecraft);
        assert_eq!(c.discord_chat_channel_truncate_length, 256);
        assert_eq!(c.discord_chat_channel_role_aliases.get("Developer").map(String::as_str), Some("Dev"));
        assert_eq!(c.discord_chat_channel_discord_filters.len(), 2);
        assert!(c.minecraft_player_join_message.enabled);
        assert_eq!(c.minecraft_player_join_message.embed.color, "#00ff00");
        assert_eq!(c.minecraft_player_join_message.embed.author.name, "%username% joined the server");
        assert_eq!(c.minecraft_player_death_message.embed.author.name, "%deathmessage%");
        assert_eq!(c.minecraft_player_achievement_message.embed.timestamp, Timestamp::Now(false));
        assert_eq!(c.server_watchdog_timeout, 30);
        assert_eq!(c.accounts_file, "accounts.aof");
        assert!(c.minecraft_discord_account_linked_use_pm);
        assert_eq!(c.minecraft_discord_account_linked_role_name_to_add_user_to, "Linked");
        assert!(c.discord_chat_channel_server_startup_message.contains("Server has started"));
    }

    #[test]
    fn a_console_only_config_has_every_chat_feature_off() {
        let c = Config::parse("DiscordConsoleChannelId = \"1\"").unwrap().chat;
        assert!(!c.has_channels());
        assert!(!c.discord_chat_channel_minecraft_to_discord);
        assert!(!c.minecraft_player_join_message.enabled);
        assert!(!c.server_watchdog_enabled);
        assert_eq!(c.channel_for("global"), None);
    }

    #[test]
    fn messages_go_to_their_own_channel_or_the_first() {
        let c = Chat {
            channels: vec![("global".into(), "10".into()), ("deaths".into(), "20".into())],
            ..Chat::default()
        };
        assert_eq!(c.channel_for("deaths"), Some("20"));
        assert_eq!(c.channel_for("join"), Some("10"));
        assert_eq!(c.own_channel("join"), None);
        assert!(c.is_chat_channel("20"));
        assert!(!c.is_chat_channel("30"));
    }

    #[test]
    fn timestamps_take_a_flag_or_an_epoch() {
        #[derive(Deserialize)]
        struct T {
            a: Timestamp,
            b: Timestamp,
        }
        let t: T = toml::from_str("a = true\nb = 1700000000").unwrap();
        assert_eq!(t.a, Timestamp::Now(true));
        assert_eq!(t.b, Timestamp::Epoch(1_700_000_000));
    }

    #[test]
    fn an_empty_whitelist_lets_no_command_through() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap();
        assert!(!c.command_allowed("say hi"));
        assert!(!c.command_allowed("stop"));
    }

    #[test]
    fn whitelist_and_blacklist_match_whole_words() {
        let mut c = Config {
            discord_console_channel_blacklist_acts_as_whitelist: true,
            discord_console_channel_blacklisted_commands: vec!["say".into(), "tps".into()],
            ..Config::default()
        };
        assert!(c.command_allowed("say hello"));
        assert!(c.command_allowed("/TPS"));
        assert!(!c.command_allowed("sayx"));
        assert!(!c.command_allowed("op someone"));
        c.discord_console_channel_blacklist_acts_as_whitelist = false;
        c.discord_console_channel_blacklisted_commands = vec!["op".into(), "execute".into()];
        assert!(!c.command_allowed("op someone"));
        assert!(c.command_allowed("deop someone"));
        assert!(c.command_allowed("say hi"));
        assert!(!c.command_allowed("   "));
    }

    #[test]
    fn levels_ignore_case() {
        let c = Config::default();
        assert!(c.level_enabled("INFO"));
        assert!(!c.level_enabled("DEBUG"));
    }
}
