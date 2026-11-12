# horizon-discordsrv

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

Not ported (see the Known gaps section): webhook delivery, group/nickname/ban synchronisation, "require linked account to play", voice, alerts,
rotating statuses, per-channel formats, `/discord reload` and `resync` (they answer only),
`language` and `debugger`. What Horizon lacks
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
  `[MinecraftPlayerJoinMessage.Embed]`, `Author = { ImageUrl, Name, Url }`); `Webhook` is not read.
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

What does not work yet on arismoko/horizon (each is marked `TODO(arismoko-api: ...)` in `src/lib.rs`):

- Off by default: set a console channel, chat channels and a token file to switch things on.
- Death lines fall back to the generic text because the killer and item names are not passed to the hook.
- Advancement announcements are not sent yet (title and announce flag are not readable).
- Only a fixed set of message keys has a line; a server in another language gets English there.

## Licence

Derived from DiscordSRV (https://github.com/DiscordSRV/DiscordSRV, GPL-3.0). This port is
licensed under the GNU General Public License v3.0 or later, see [LICENSE](LICENSE) and
[NOTICE](NOTICE). The commands, messages, config keys and the chat relay follow DiscordSRV 1.30.5's
source (`plugin.yml`, `commands/*`, `listeners/DiscordChatListener`, `DiscordSRV.processChatMessage`,
`messages.yml`, `config.yml`, `linking/*`).

Versions up to 0.2.x were written without reading DiscordSRV's source (from its public config files
and Discord's API documentation); 0.3.0 re-checks everything against the source.
