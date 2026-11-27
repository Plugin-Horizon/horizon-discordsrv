//! `config.toml`: the keys DiscordSRV's `config.yml`, `messages.yml` and the account linking
//! part of `linking.yml` use, under the same names. The console channel keys are [`Config`]'s
//! own; the chat channels, notifications, linking and watchdog keys are [`Chat`]'s, flattened
//! in, and all of them off when missing (a console-only config stays console-only).
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The file written when none exists: the bridge stays off until a console channel is set.
pub const DEFAULT_CONFIG: &str = include_str!("../../data/discordsrv/config.example.toml");

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
    /// `ProxyHost`, `ProxyPort`, `ProxyUser`, `ProxyPassword`: the HTTP proxy for Discord's REST API and
    /// gateway (see [`Config::proxy`]).
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_user: String,
    pub proxy_password: Secret,
    /// Chat channels, notifications, linking, watchdog (missing keys: off).
    #[serde(flatten)]
    pub chat: Chat,
    /// voice.yml (missing keys: off).
    #[serde(flatten)]
    pub voice: VoiceSettings,
    /// `EnablePresenceInformation`: ask Discord for the (privileged) presences intent, so the placeholders about what
    /// a member is doing (`user_online_status`, `user_game_name`, `linked_online` ...) have an answer. Off by default:
    /// the intent must be switched on for the bot in the Developer Portal first.
    pub enable_presence_information: bool,
    /// `Experiment_JdbcAccountLinkBackend`: `jdbc:mysql://HOST:PORT/DATABASE?...`; the placeholder URL means off.
    #[serde(rename = "Experiment_JdbcAccountLinkBackend")]
    pub jdbc_account_link_backend: String,
    #[serde(rename = "Experiment_JdbcTablePrefix")]
    pub jdbc_table_prefix: String,
    #[serde(rename = "Experiment_JdbcUsername")]
    pub jdbc_username: String,
    #[serde(rename = "Experiment_JdbcPassword")]
    pub jdbc_password: Secret,
    /// `DisabledPluginHooks`: hooks that are not used, by name (case does not matter): `LuckPerms-GroupUpdates`,
    /// `LuckPerms-Contexts` (or `LuckPerms` for both), `PlaceholderAPI`, `Multiverse-Core`, `SuperVanish`, `Vault`.
    pub disabled_plugin_hooks: Vec<String>,
}

/// The keys of voice.yml, under the original's names.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceSettings {
    #[serde(rename = "Voice enabled")]
    pub enabled: bool,
    /// Game ticks between voice checks (default 5).
    #[serde(rename = "Tick speed")]
    pub tick_speed: u64,
    #[serde(rename = "Voice category")]
    pub category: String,
    #[serde(rename = "Lobby channel")]
    pub lobby: String,
    #[serde(rename = "Mute users who bypass speak permissions in the lobby")]
    pub mute_bypassers: bool,
    #[serde(rename = "Network")]
    pub network: VoiceNetwork,
}

/// The `Network` section of voice.yml.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceNetwork {
    #[serde(rename = "Vertical Strength")]
    pub vertical_strength: f64,
    #[serde(rename = "Horizontal Strength")]
    pub horizontal_strength: f64,
    #[serde(rename = "Falloff")]
    pub falloff: f64,
    #[serde(rename = "Channels are visible")]
    pub channels_visible: bool,
}

impl Default for VoiceNetwork {
    fn default() -> Self {
        VoiceNetwork { vertical_strength: 40.0, horizontal_strength: 80.0, falloff: 5.0, channels_visible: false }
    }
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            enabled: false,
            tick_speed: 5,
            category: String::new(),
            lobby: String::new(),
            mute_bypassers: true,
            network: VoiceNetwork::default(),
        }
    }
}

/// A password: kept out of `Debug` output.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(empty)" } else { "Secret(..)" })
    }
}

/// The proxy of `ProxyHost` and its siblings.
#[derive(Clone)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Proxy({}:{})", self.host, self.port)
    }
}

impl Proxy {
    /// `Proxy-Authorization` for a CONNECT: Basic with the user and password, when a password is set
    /// (DiscordSRV sends credentials only then).
    pub fn authorization(&self) -> Option<String> {
        use base64::Engine;
        (!self.password.is_empty()).then(|| {
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.user.trim(), self.password.trim())))
        })
    }

    /// The `[user:password@]host:port` form `ureq` reads.
    pub fn ureq_url(&self) -> String {
        let auth = if self.password.is_empty() { String::new() } else { format!("{}:{}@", self.user.trim(), self.password.trim()) };
        format!("{auth}{}:{}", self.host.trim(), self.port)
    }
}

impl Config {
    /// The proxy to use: a host that is set and is not the file's `example.com` placeholder.
    pub fn proxy(&self) -> Option<Proxy> {
        let host = self.proxy_host.trim();
        (!host.is_empty() && host != "example.com" && self.proxy_port != 0).then(|| Proxy {
            host: host.to_string(),
            port: self.proxy_port,
            user: self.proxy_user.clone(),
            password: self.proxy_password.0.clone(),
        })
    }
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
    /// `MinecraftMentionSound` (default `true` in DiscordSRV): a Discord message that mentions an
    /// online player as `@name` plays the note-block pling to them. Missing here: off.
    pub minecraft_mention_sound: bool,
    /// `ForcedLanguage`: only English exists in this port; any other value is warned about.
    pub forced_language: String,
    /// `Timezone`: `default` (the server's) is the only one applied; any other value is warned about.
    pub timezone: String,
    /// `Debug`: the categories of DiscordSRV's `Debug` enum logged all the time (`/discord debugger` adds more).
    pub debug: Vec<String>,
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
    /// `Require linked account to play` (linking.yml): logins need a linked account, and may need
    /// membership of the Discord server, a subscriber role, with the whitelist and ban rules.
    /// IP bans are not read: the login hook has no address (see the README).
    #[serde(rename = "Require linked account to play")]
    pub require_linked_account_to_play: RequireLinked,
    pub discord_chat_channel_block_bots: bool,
    pub discord_chat_channel_block_webhooks: bool,
    /// `Experiment_MCDiscordReserializer_ToDiscord`: formatting codes in game chat become Discord markdown
    /// (bold, italic, underline, strikethrough) instead of being stripped.
    #[serde(rename = "Experiment_MCDiscordReserializer_ToDiscord")]
    pub reserializer_to_discord: bool,
    /// `Experiment_MCDiscordReserializer_ToMinecraft`: those four markdown formats in Discord messages
    /// become formatting codes in the game.
    #[serde(rename = "Experiment_MCDiscordReserializer_ToMinecraft")]
    pub reserializer_to_minecraft: bool,
    /// `Experiment_MCDiscordReserializer_InBroadcast`: `/discord broadcast` turns codes into markdown.
    #[serde(rename = "Experiment_MCDiscordReserializer_InBroadcast")]
    pub reserializer_in_broadcast: bool,
    /// alerts.yml: the `Alerts` list, read by [`crate::discordsrv_bridge::alerts::parse`].
    #[serde(rename = "Alerts")]
    pub alerts: Vec<serde_json::Value>,
    /// synchronization.yml: group name -> role ID (all-zero IDs and empty names are skipped).
    pub group_role_synchronization_groups_and_roles_to_sync: HashMap<String, String>,
    #[serde(default = "yes")]
    pub group_role_synchronization_minecraft_is_authoritative: bool,
    pub group_role_synchronization_one_way: bool,
    /// `GroupRoleSynchronizationEnableDenyPermission`: `discordsrv.sync.deny.<group>` keeps a player out of a synchronised group.
    pub group_role_synchronization_enable_deny_permission: bool,
    pub group_role_synchronization_primary_group_only: bool,
    #[serde(default = "yes")]
    pub group_role_synchronization_on_link: bool,
    /// Minutes between the synchronizations of the players on the server (at least 1).
    #[serde(default = "five")]
    pub group_role_synchronization_cycle_time: u64,
    /// `GroupRoleSynchronizationCycleCompletely`: the timer synchronises every linked player, not only those online.
    pub group_role_synchronization_cycle_completely: bool,
    /// `NicknameSynchronizationCycleTime`: minutes between nickname synchronisations (at least 3).
    #[serde(default = "three")]
    pub nickname_synchronization_cycle_time: u64,
    /// `NicknameSynchronizationEnabled`: the linked Discord member's nickname follows the player's name.
    pub nickname_synchronization_enabled: bool,
    /// `%username%` and `%displayname%` (both the player's name); cut to Discord's 32 characters.
    #[serde(default = "nick_format")]
    pub nickname_synchronization_format: String,
    pub ban_synchronization_discord_to_minecraft: bool,
    #[serde(default = "ban_reason")]
    pub ban_synchronization_discord_to_minecraft_reason: String,
    pub ban_synchronization_minecraft_to_discord: bool,
    /// `Experiment_WebhookChatMessageDelivery` (default false): a player's chat goes out through the
    /// bot's webhook in the chat channel (named `DiscordSRV`) instead of a message of the bot.
    #[serde(rename = "Experiment_WebhookChatMessageDelivery")]
    pub experiment_webhook_chat_message_delivery: bool,
    /// `Experiment_WebhookChatMessageUsernameFormat`; empty: `%displayname%`.
    #[serde(rename = "Experiment_WebhookChatMessageUsernameFormat")]
    pub experiment_webhook_chat_message_username_format: String,
    /// `Experiment_WebhookChatMessageFormat`; empty: `%message%`.
    #[serde(rename = "Experiment_WebhookChatMessageFormat")]
    pub experiment_webhook_chat_message_format: String,
    /// `Experiment_WebhookChatMessageUsernameFilters`: ordered `[regex, replacement]` pairs run over the webhook username.
    #[serde(rename = "Experiment_WebhookChatMessageUsernameFilters")]
    pub experiment_webhook_chat_message_username_filters: Vec<(String, String)>,
    /// `Experiment_WebhookChatMessageUsernameFromDiscord`: the linked Discord member's name.
    #[serde(rename = "Experiment_WebhookChatMessageUsernameFromDiscord")]
    pub experiment_webhook_chat_message_username_from_discord: bool,
    /// `Experiment_WebhookChatMessageAvatarFromDiscord`: the linked Discord member's avatar.
    #[serde(rename = "Experiment_WebhookChatMessageAvatarFromDiscord")]
    pub experiment_webhook_chat_message_avatar_from_discord: bool,
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

fn nick_format() -> String {
    "%displayname%".to_string()
}

fn three() -> u64 {
    3
}

fn five() -> u64 {
    5
}

fn yes() -> bool {
    true
}

fn ban_reason() -> String {
    "&cYou have been banned until further notice from the server because you were banned on our Discord server."
        .to_string()
}

/// The `Require linked account to play` section of linking.yml.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RequireLinked {
    #[serde(rename = "Enabled")]
    pub enabled: bool,
    /// Minecraft names let in without a link.
    #[serde(rename = "Bypass names")]
    pub bypass_names: Vec<String>,
    /// Kick text of a player without a link (`{BOT}`, `{CODE}`, `{INVITE}`, `&` codes); empty: the default.
    #[serde(rename = "Not linked message")]
    pub not_linked_message: String,
    /// `Listener priority` and `Listener event`: which Bukkit event DiscordSRV checks on. Horizon has
    /// one login hook, so both are read and have no effect.
    #[serde(rename = "Listener priority")]
    pub listener_priority: String,
    #[serde(rename = "Listener event")]
    pub listener_event: String,
    /// Players on the vanilla whitelist skip the check (default true when the key is missing).
    #[serde(rename = "Whitelisted players bypass check")]
    pub whitelisted_players_bypass_check: Option<bool>,
    /// Banned players are checked too (default: they skip it).
    #[serde(rename = "Check banned players")]
    pub check_banned_players: bool,
    /// Only banned players need to link.
    #[serde(rename = "Only check banned players")]
    pub only_check_banned_players: bool,
    /// `true`/`false`: in at least one Discord server the bot is in; a server ID; or a list of IDs
    /// (all of them). Missing: `true`, as in DiscordSRV's file.
    #[serde(rename = "Must be in Discord server")]
    pub must_be_in_discord_server: serde_json::Value,
    #[serde(rename = "Subscriber role")]
    pub subscriber_role: SubscriberRole,
    #[serde(rename = "Messages")]
    pub messages: RequireLinkedMessages,
}

/// `Require linked account to play.Subscriber role`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SubscriberRole {
    #[serde(rename = "Require subscriber role to join")]
    pub required: bool,
    #[serde(rename = "Subscriber roles")]
    pub roles: Vec<String>,
    #[serde(rename = "Require all of the listed roles")]
    pub require_all: bool,
    #[serde(rename = "Kick message")]
    pub kick_message: String,
}

/// `Require linked account to play.Messages.Kicked for unlinking`, English default.
pub const KICKED_FOR_UNLINKING: &str =
    "&cYou have been kicked from the server for unlinking your accounts.\n\nPlease rejoin the server to link your accounts again.";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RequireLinkedMessages {
    /// Kick text while the bridge has not connected to Discord yet; empty: the default.
    #[serde(rename = "DiscordSRV still starting")]
    pub still_starting: String,
    /// Kick text of a player who unlinks while linking is required; empty: the default.
    #[serde(rename = "Kicked for unlinking")]
    pub kicked_for_unlinking: String,
    #[serde(rename = "Not in server")]
    pub not_in_server: String,
    #[serde(rename = "Failed to find subscriber role")]
    pub failed_to_find_subscriber_role: String,
    #[serde(rename = "Failed for unknown reason")]
    pub failed_for_unknown_reason: String,
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

/// A notification of messages.yml (`MinecraftPlayerJoinMessage` and its siblings).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MessageTemplate {
    pub enabled: bool,
    pub content: String,
    pub embed: EmbedTemplate,
    pub webhook: NoticeWebhook,
}

/// The `Webhook` section of a notification: sent under this name and avatar through a webhook
/// (`Enable` is DiscordSRV's other spelling of `Enabled`). Empty name and avatar: the bot's.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct NoticeWebhook {
    pub enabled: bool,
    pub enable: bool,
    pub name: String,
    pub avatar_url: String,
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
            proxy_host: String::new(),
            proxy_port: 0,
            proxy_user: String::new(),
            proxy_password: Secret::default(),
            chat: Chat::default(),
            voice: VoiceSettings::default(),
            enable_presence_information: false,
            jdbc_account_link_backend: String::new(),
            jdbc_table_prefix: String::new(),
            jdbc_username: String::new(),
            jdbc_password: Secret::default(),
            disabled_plugin_hooks: Vec::new(),
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
    fn the_webhook_keys_read_their_original_names() {
        let text = format!("{DEFAULT_CONFIG}\n[MinecraftPlayerJoinMessage.Webhook]\nEnable = true\nName = \"Server\"\n");
        let c = Config::parse(&text).unwrap().chat;
        assert!(!c.experiment_webhook_chat_message_delivery);
        assert_eq!(c.experiment_webhook_chat_message_username_format, "%displayname%");
        assert_eq!(c.experiment_webhook_chat_message_format, "%message%");
        assert!(!c.experiment_webhook_chat_message_avatar_from_discord);
        assert!(c.minecraft_player_join_message.webhook.enable);
        assert!(!c.minecraft_player_join_message.webhook.enabled);
        assert_eq!(c.minecraft_player_join_message.webhook.name, "Server");
    }

    #[test]
    fn the_proxy_is_off_for_the_placeholder_and_never_printed() {
        let mut c = Config::default();
        assert!(c.proxy().is_none());
        c.proxy_host = "example.com".into();
        c.proxy_port = 1234;
        assert!(c.proxy().is_none(), "the file's placeholder host is no proxy");
        c.proxy_host = "proxy.internal".into();
        c.proxy_user = "u".into();
        c.proxy_password = Secret("hunter2pw".into());
        let p = c.proxy().unwrap();
        assert_eq!(p.ureq_url(), "u:hunter2pw@proxy.internal:1234");
        assert_eq!(p.authorization().as_deref(), Some("Basic dTpodW50ZXIycHc="));
        assert!(!format!("{c:?}{p:?}").contains("hunter2pw"));
        c.proxy_password = Secret::default();
        assert_eq!(c.proxy().unwrap().ureq_url(), "proxy.internal:1234");
        assert_eq!(c.proxy().unwrap().authorization(), None);
    }

    #[test]
    fn the_require_linked_section_reads_its_original_names() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap().chat;
        assert!(!c.require_linked_account_to_play.enabled);
        assert!(c.require_linked_account_to_play.bypass_names.is_empty());
        let text = DEFAULT_CONFIG.replace("Enabled = false\n\"Bypass names\" = []", "Enabled = true\n\"Bypass names\" = [\"Scarsz\"]");
        assert_ne!(text, DEFAULT_CONFIG);
        let on = Config::parse(&text).unwrap().chat.require_linked_account_to_play;
        assert!(on.enabled);
        assert_eq!(on.bypass_names, vec!["Scarsz".to_string()]);
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
