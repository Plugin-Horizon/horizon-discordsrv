# horizon-discordsrv

## Port status

Compared with DiscordSRV 1.30.x (item list audit/rec100/discordsrv.md, round 5, 2026-11-25): every item is done, n/a for a reason that only the JVM gives (update check, Bukkit plugin events, JVM TLS/DNS keys), or waiting for a workspace change (the MySQL account-link backend). Status: round 5 not yet compiled.

> **Unofficial Rust port of DiscordSRV for the Horizon server (https://github.com/arismoko/horizon). Not affiliated with or endorsed by the DiscordSRV authors.**

Not affiliated with DiscordSRV.

> **Built for and working only with [arismoko/horizon](https://github.com/arismoko/horizon)** (the Horizon fork that carries the mod API). [horizon-rust/horizon](https://github.com/horizon-rust/horizon) is experimental for mods and not supported: the earlier version for its native hooks / horizon-mixins is frozen on the `horizon-plain` branch (work stopped on 2026-11-12).

## Which Horizon

This is a native trait mod: `horizon-mod.toml`, a `Cargo.toml` that depends on `horizon-mod-api`, `src/lib.rs` with the hooks, and the plugin logic in the `core/` crate (plain Rust, no engine needed for its tests).

- Pin: `[native] horizon = "54090ee36c4e2c7b43b66db864220c577b94b7e0"` in `horizon-mod.toml` (game `26.3`). That is a development build of arismoko/horizon `main` plus mod API additions that are not all merged upstream yet, so a stock `main` may lack some host methods this mod calls until they land.
- It compiled and loaded on that build and was smoke-tested there; it has not seen a long run with real players. Treat it as experimental.
- Feature descriptions below were written for the earlier horizon-rust version; where they name engine calls (`native_*`) or old file paths, read them as descriptions only. The Known gaps list is current.

## What it does

- **Console forwarding**: hears every message the log gets (hook `native_log_line`, from the
  run's first line, so no file is tailed), keeps the
  lines of the configured levels, runs them through the regex filters, adds the configured
  prefix/suffix, and posts them every `DiscordConsoleChannelLogRefreshRateInSeconds` seconds,
  packed into as few messages as fit Discord's 2000 characters, in code blocks when
  `DiscordConsoleChannelUseCodeBlocks` is on. Mentions in forwarded lines never ping anyone.
- **Commands from the channel**: a message in the console channel is run as a console command
  on the next server tick when the command list allows it, its answer posted back to the
  channel (`native_run_command_captured`); another mod can rewrite or refuse a message first
  with `set_console_preprocessor(f(author_id, command) -> Preprocess)`, which runs before the
  command list (a blacklist, or a whitelist when
  `DiscordConsoleChannelBlacklistActsAsWhitelist` is on; an empty whitelist allows nothing).
  Bots and webhooks are ignored when `DiscordConsoleChannelBlockBots` is on. Each command is
  written to the usage log first; with `CancelConsoleCommandIfLoggingFailed` a command whose
  log line can't be written does not run.
- **Presence**: the first `DiscordGameStatus` entry ("playing X", "watching X",
  "listening to X", "competing in X") and `DiscordOnlineStatus`, sent with the gateway login.
- **Channel topic**: `ChannelTopicUpdaterConsoleChannelTopicFormat` five seconds after start
  and every `ChannelTopicUpdaterRateInMinutes`; the shutdown format when the server stops
  (if `ChannelTopicUpdaterChannelTopicsAtShutdownEnabled`).
- **Gateway**: Discord gateway v10 (JSON, no compression) over the websocket protocol of the
  locked `tungstenite` and TLS of the locked `rustls` (the HTTP upgrade is written in the mod:
  the locked tungstenite has no handshake feature). Heartbeats with ACK check, Resume after a
  drop or an op 7 Reconnect, a new session after op 9 / close 4007 / 4009, and it gives up on
  4004 (bad token) and 4010 to 4014 (shards, version, intents). REST through the locked `ureq`:
  `GET /gateway/bot`, `POST /channels/{id}/messages`, `PATCH /channels/{id}`, 429s wait their
  `retry_after`.
- **Chat channels** (`Channels`, ordered `[game channel, Discord ID]` pairs; the first is the
  default, `global`/`status`/`awards`/`deaths`/`join`/`leave`/`watchdog`/`link` take their kind):
  - game chat to Discord in `MinecraftChatToDiscordMessageFormat`, names escaped, game filters,
    the prefix rule, `@name`/`#channel` turned into mentions where the bot knows them, only the
    `DiscordChatChannelAllowedMentions` kinds pinging;
  - Discord messages in game in `DiscordToMinecraftChatMessageFormat` (top role, alias,
    approximated colour, all roles, reply), with the blocked bots/webhooks/IDs/roles, roles
    selection, colour codes only for the listed roles, Discord filters, truncation, emotes as
    `:name:`, attachments as URLs; the bot's own messages are never echoed;
  - join, first join, leave, death and advancement notifications as the embeds of
    `messages.yml`; the startup message after the first tick, and at a stop the leave messages
    of the players still on, then the shutdown message;
  - the `playerlist` command (answer deleted after `DiscordChatChannelListCommandExpiration`,
    with the request), canned responses, the chat channel topic, `ChannelUpdater` channel names,
    and the watchdog (`ServerWatchdogMessage` when the server hasn't ticked for
    `ServerWatchdogTimeout` seconds).
- **Account linking**: `/discord link` gives a four-digit code, sent to the bot by PM (or in
  the `link` channel when `MinecraftDiscordAccountLinkedUsePM` is off) it links the accounts,
  gives the `MinecraftDiscordAccountLinkedRoleNameToAddUserTo` role and runs the linked console
  commands; `/discord linked`, `/discord unlink` (operators only), `/discord help`, `/discord`
  (invite). Operators also get `/discord link <player> <discord>`, `linked <player>`,
  `unlink <player>` and `broadcast <#channel> <message>`; `resync` and `reload` answer only.
  `DiscordChatChannelRequireLinkedAccount` keeps unlinked users' messages out of the game.
- All network I/O runs on threads of the mod's own (`DiscordSRV - Gateway`,
  `DiscordSRV - Console`, `DiscordSRV - Chat`); the server thread stores the TPS and a player
  snapshot every second, hands over the game's events, and takes queued commands and lines.
  A stop waits five seconds at most for the last messages and topics.

- **`!c` console commands from a chat channel** (`DiscordChatChannelConsoleCommand*`: role list,
  whitelist/blacklist with bypass roles, usage log, answers posted back, expiry).
- Canned responses start a message (any case) and answer in any channel the bot reads; the
  player list is the code block of DiscordSRV (`n/max`, sorted); a message of an unlinked author
  is deleted; blocked roles get a cross, truncated messages a speech bubble and an exclamation mark;
  each attachment (three at most) and sticker is a line of its own.

Webhook delivery is ported (see Config). Not ported (see the Known gaps section): Discord -> Minecraft group
synchronisation, voice,
rotating statuses, per-channel formats, `/discord resync` (it answers only). What Horizon lacks
for the rest is in the Known gaps section.

## Config

The first start writes `config/discordsrv/config.toml` (beside `server.properties`) from
[`bridge/config.example.toml`](bridge/config.example.toml) and stays off until
`DiscordConsoleChannelId` or a chat channel in `Channels` is set. Keys carry DiscordSRV's names
(`config.yml`, `messages.yml`, `linking.yml`); a key left out is off, so a console-only config
of 0.1.0 works unchanged. Differences:

- `BotTokenFile` (default `token.txt`, relative to `config/discordsrv/`) names a file holding
  the token; the token is never in the config. Keep that file out of any repository.
- `DiscordConsoleChannelFilters` is an ordered list of `[regex, replacement]` pairs. Java's
  `$1` group references work.
- `{date}` in the prefix uses `DiscordConsoleChannelTimestampFormat` (Java `SimpleDateFormat`
  letters `y M d E H h m s S a z Z` and quoted text). `{name}`, the Java logger's name, is
  always empty: Horizon's log has threads, not loggers.
- Topic placeholders: `%tps%`, `%serverversion%` (`Horizon (MC: <version>)`), `%date%`,
  `%time%`, and the memory ones mapped onto a native process: used = the server's resident
  memory, free = the machine's available memory, max = its total memory.
- `ApiBaseUrl` (default `https://discord.com/api/v10`) exists for tests against a local fake.
- `Channels`, `DiscordConsoleChannelFilters`, `DiscordChatChannelGameFilters` and
  `DiscordChatChannelDiscordFilters` are ordered lists of pairs (TOML tables have no order).
- Formats take MiniMessage colour/decoration tags (`<aqua>`, `</aqua>`, `<reset>`, `<#55ffff>`,
  approximated to the sixteen chat colours) and `&` codes.
- Notifications are TOML tables named like messages.yml's (`[MinecraftPlayerJoinMessage]`,
  `[MinecraftPlayerJoinMessage.Embed]`, `Author = { ImageUrl, Name, Url }`); a `Webhook` table (`Enable`, `Name`,
  `AvatarUrl`) sends the notice through the bot's webhook under that name and avatar.
- `AvatarUrl` empty means `https://crafatar.com/avatars/{uuid-nodashes}?size={size}&overlay`.
- `AccountsFile` (default `accounts.aof`, beside the config) holds the links, one
  `<discord id> <uuid>` line each.
- `%primarygroup%` is always empty and `%displayname%` is the player's name (no permissions or
  nickname API between mods yet).

The bot needs the **Message Content** intent (Developer Portal, Bot, Privileged Gateway
Intents) to read commands and chat, and permission to view the channels, send messages, embed
links, manage messages (the player list's expiry), manage the channels (topics, names) and
manage roles (the linked role). The privileged members intent is not used: members are learned
from their messages.

## Build

Build it as a native trait mod with arismoko's launcher (see the "native mods" how-to in the arismoko/horizon docs):

```sh
horizon-launcher native-build <path to this folder> --server-output <dir> --version <tag> --trust-native-code
```

`--trust-native-code` is your consent to compile and run this code inside the server. The logic crate tests run without the engine: `(cd core && cargo test)`.

## Known gaps

What does not work yet on arismoko/horizon (round 2: the `TODO(arismoko-api` markers are gone):

- Off by default: set a console channel, chat channels and a token file to switch things on.
- Webhook delivery (`Experiment_WebhookChatMessageDelivery`): the bot makes a webhook named `DiscordSRV` in each
  chat channel it posts to (it needs Manage Webhooks) and keeps it for the run; webhook URLs are never written to a
  file or a log. `Experiment_WebhookChatMessageUsernameFilters` is read by the original but never applied there, so it is not read here.
- `Require linked account to play` (linking.yml): an unlinked player is refused at login with the kick text (`{BOT}`,
  `{CODE}`, `{INVITE}`), bypass names and the still-starting text are read. Not checked: the Discord server membership
  (`Must be in Discord server`, on by default in the original), the subscriber roles, the vanilla whitelist, the ban
  checks (the kick on unlink is done).
- Death notices use the message the game built (`player_death_message`: killer and weapon included), flattened through `host.translate` (the server's own language) and, when the engine gives no translation, a small built-in English table plus names made from `entity.`/`item.`/`block.minecraft.*` keys. No Mojang language file is shipped. Advancement announcements use `host.advancement(id)` (title, announce flag) and the same lookup for translated titles; a title key outside vanilla shows as its key. 
- Only a fixed set of message keys has a line; a server in another language gets English there.
- `/discord reload` re-reads the config and restarts the bridge (the gateway reconnects; no shutdown or startup message). `/discord resync` runs the Minecraft -> Discord group synchronisation (`GroupRoleSynchronization*` keys in config.toml; groups come from the permission mod through `permission_groups`; it answers DiscordSRV's "requires valid ... entries" line when no group has a role ID). Not possible from a mod: Discord -> Minecraft group changes and `discordsrv.sync.<group>` permissions (engine need `permission_group_write`).
- Ban synchronisation (`BanSynchronizationDiscordToMinecraft`, `...MinecraftToDiscord`, both off by default as in the original): a Discord ban runs `ban`/`pardon` as the console, a server ban bans the linked Discord account, a joining player's Discord ban is lifted. A player who unlinks while linking is required is kicked ("Kicked for unlinking"). `/discord debugger` switches the debug categories (`[<CATEGORY> DEBUG]` log lines for chat in both directions; the `Debug` config list works too); `/discord debugger upload` writes a report file under `config/discordsrv/debug/` instead of uploading it to a paste service. `/discord debug` does not exist in DiscordSRV 1.30 either (only its usage text mentions it): it answers "That command doesn't exist!".
- Config keys read: `MinecraftMentionSound` (the note-block pling to an online player a Discord message mentions as `@name`), `ForcedLanguage` and `Timezone` (only `none`/English and `default` are applied; others warn), `Debug`, the webhook keys and the `Webhook` part of the notices. Not read: `Experiment_Jdbc*`, `ForceTLSv12`, `NoopHostnameVerifier`, `MaximumAttemptsForSystemDNSBeforeUsingFallbackDNS`, `UseModernPaperChatEvent`, `VentureChatBungee`. Read since round 5: `DisabledPluginHooks`, `EnablePresenceInformation`, the voice keys.
- Nickname synchronisation (`NicknameSynchronization*`, Minecraft name -> Discord nickname, on the resync cycle) is done.
- **Voice** (`voice.yml`, round 5): the voice module of the original: `Voice enabled`, `Tick speed`, `Voice category`, `Lobby channel`, `Mute users who bypass speak permissions in the lobby` and the `Network` strengths, falloff and visibility. Players who are linked and sit in the lobby or the category are put into a shared voice channel when two are within the horizontal and vertical strength, join one when in range of it, leave it beyond strength plus falloff, networks that touch merge, an emptied channel is deleted, the lobby denies @everyone speaking, and channels an earlier run left behind are emptied and deleted. It needs the bot permissions the original names (an error line says which is missing, every 30 seconds) and the gateway's voice-states intent, which is requested when voice is on. Differences: the server thread samples positions every `Tick speed` ticks (a moved block marks a player), a move that was just asked for is not asked again for three seconds, and the missing-permission line repeats every 30 seconds (the original stamped its timer on every stopped tick, so it logged once).
- **Group synchronisation Discord -> Minecraft** (round 5): `GroupRoleSynchronizationOneWay` / `...MinecraftIsAuthoritative` decide who wins as in the original. A role edit (GUILD_MEMBERS intent, requested only when Discord can win), the cycle, a link and `/discord resync` read the member's roles and give or take the groups; with `PrimaryGroupOnly` a group is only given. There is no provider-neutral write on Horizon (ENGINE_NEEDS `permission_group_write`), so the write runs the permissions provider's console command (`lp user <uuid> parent add|remove <group>`, LuckPerms' syntax). `discordsrv.sync.<group>` (the player counts as in the group) and, with `GroupRoleSynchronizationEnableDenyPermission`, `discordsrv.sync.deny.<group>` (they do not) are nodes declared for each synchronised group and read through the permission handler. The engine call `permission_group_write` (ENGINE_NEEDS) would replace the console command.
- **Plugin hooks** (round 5), by Horizon mod: LuckPerms instant group updates (the `permission_groups_changed` hook updates the roles at once) and LuckPerms contexts (service `discordsrv:contexts`, `key=value` lines: `discordsrv:linked`, `:server_id`, `:boosting`, `:role`, `:role_id`); the PlaceholderAPI expansion (service `discordsrv:placeholder`, payload `<uuid hex or -> <identifier>`, every `%discordsrv_...%` value; presence values need `EnablePresenceInformation`, `guild_members_online` is the guild's approximate count refreshed once a minute); Vault's primary group and groups (`permission_groups`, also `%primarygroup%` in the list command and chat formats); vanish (the engine's `hide`, which SuperVanish uses: no join or quit notice, not in the player list or `%playercount%`, no nickname update; services `discordsrv:fake_join` / `discordsrv:fake_quit`, payload `<uuid hex> <message>`, for SuperVanish's fake messages); Multiverse world aliases (`%worldalias%`, from the service `multiverse:world_alias` when that mod provides it, else the world name). `DisabledPluginHooks` switches each off by name (`LuckPerms-GroupUpdates`, `LuckPerms-Contexts`, `LuckPerms`, `PlaceholderAPI`, `Multiverse-Core`, `SuperVanish`, `Vault`). Another mod must call a service for it to be used: LuckPerms (contexts), PlaceholderAPI (placeholders), SuperVanish (fake join/quit) and Multiverse (provides the alias).
- **Login checks** ("Require linked account to play"): in DiscordSRV's order: `Bypass names`, `Whitelisted players bypass check` (the server's whitelist), `Check banned players` / `Only check banned players` (the server's name ban list), the "still starting" text, the link (a code is made for an unlinked player), `Must be in Discord server` (`true`: a guild the bot is in; a server ID; or a list that must all hold; checked by REST), the subscriber roles (`Require subscriber role to join`, `Subscriber roles`, `Require all of the listed roles`, `Kick message`), and the messages `Not in server`, `Failed to find subscriber role`, `Failed for unknown reason`. A Discord error refuses the login with the last text, as DiscordSRV's catch-all does. A player who unlinks is kicked unless the whitelist bypass or "only banned players" holds. The IP ban list is not looked at: the login hook has no address, and behind the proxy every login comes from the proxy. `Listener priority` / `Listener event` are read and have no effect (one login hook).
- **Alerts** (`alerts.yml`, or `[[Alerts]]` in the TOML file): `Trigger` is `PlayerJoinEvent`, `PlayerQuitEvent`, `PlayerDeathEvent`, `PlayerAdvancementDoneEvent`, `AsyncPlayerChatEvent`/`AsyncChatEvent`, `PlayerCommandPreprocessEvent`, `ServerCommandEvent` (simple or package-qualified names) or `/command`. `Conditions` and `${...}` are an expression subset instead of Spring's language: literals, `#player` (`name`, `displayName`, `uniqueId`, `world.name`, `gameMode`, `ping`, `hasPermission('x')`), `#args`, `#allArgs`, `#command`, `#server.onlinePlayers`, the event's `message`, `== != < > <= >=`, `&& || !`, and the common string and list methods. As in DiscordSRV an error in a condition is logged and the condition counts as met; an expression that fails stays as written. `Channel` is a game channel of `Channels`, a Discord channel name or an ID; `Content`, `Embed` and `Webhook` work as in the join notice, with `{tps} {time} {date} {name} {ping} {username} {displayname} {world} {embedavatarurl} {botavatarurl} {botname}` filled. Triggers naming other Bukkit or plugin events are reported at start and never fire. `PlaceholderAPI` placeholders are not read.
- **Reserializer**: `Experiment_MCDiscordReserializer_ToDiscord` (game chat: bold, italic, underline, strikethrough codes become markdown instead of being stripped), `..._ToMinecraft` (those four markdown formats in Discord messages become codes), `..._InBroadcast` (`/discord broadcast`).
- **Proxy**: `ProxyHost`, `ProxyPort`, `ProxyUser`, `ProxyPassword` (the file's `example.com` is no proxy): REST through the HTTP proxy, the gateway through a `CONNECT` tunnel with Basic credentials. Special characters (`:`, `@`, `/`) in the user or password are not supported for REST.
- Not applicable on Horizon (the JVM gives the reason): `discordsrv.updatenotification` (the jar update checker), `VentureChatBungee` and `UseModernPaperChatEvent` (Bukkit chat plugin events), the Dynmap web-chat and other chat-plugin hooks (Bukkit plugin events), `ForceTLSv12`, `NoopHostnameVerifier` and `MaximumAttemptsForSystemDNSBeforeUsingFallbackDNS` (workarounds for the JVM's TLS defaults, trust store and DNS cache; rustls and the system resolver do not have them).
- **JDBC account links** (`Experiment_JdbcAccountLinkBackend`, `...TablePrefix`, `...Username`, `...Password`): a `jdbc:mysql://host:port/database` URL (the `HOST`/`PORT`/`DATABASE` placeholder means off) keeps the links in `<prefix>_accounts` (`link`, `discord` varchar(32) unique, `uuid` varchar(36) unique; `<prefix>_codes` is created too), read at start and every minute so other servers' links show up. A failed connection is logged and `accounts.aof` is used. The URL's parameters (`useSSL`, `autoReconnect`) are ignored: no TLS to the database.

## Licence

Derived from DiscordSRV (https://github.com/DiscordSRV/DiscordSRV, GPL-3.0). This port is
licensed under the GNU General Public License v3.0 or later, see [LICENSE](LICENSE) and
[NOTICE](NOTICE). The commands, messages, config keys and the chat relay follow DiscordSRV 1.30.5's
source (`plugin.yml`, `commands/*`, `listeners/DiscordChatListener`, `DiscordSRV.processChatMessage`,
`messages.yml`, `config.yml`, `linking/*`).

Versions up to 0.2.x were written without reading DiscordSRV's source (from its public config files
and Discord's API documentation); 0.3.0 re-checks everything against the source.
