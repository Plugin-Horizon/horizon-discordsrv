> **Frozen branch (`horizon-plain`).** This is the version written for horizon-rust/horizon's native hooks / horizon-mixins. We stopped working on it on 2026-11-12: horizon-rust moved all mod API work to the fork [arismoko/horizon](https://github.com/arismoko/horizon) (see "Mod-related pull requests" in its CONTRIBUTING.md) and closed mod pull requests there. This branch is experimental and unsupported. The maintained version, for arismoko/horizon only, is on the default branch.

# horizon-discordsrv

> **Unofficial Rust port of DiscordSRV for the Horizon server (https://github.com/horizon-rust/horizon). Not affiliated with or endorsed by the DiscordSRV authors.**

A Horizon native mod (horizon-mixins bundle) that gives a Horizon dedicated server what the
DiscordSRV plugin does: the **console channel** (the server's log streamed to a Discord channel,
commands typed there run by the console), the **chat channels** (game chat both ways, join,
leave, death and advancement notifications, start/stop messages, the player list, topics,
channel names, the watchdog), **account linking**, and the bot's presence.

Not affiliated with DiscordSRV.

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

Not ported (see [STATUS.md](STATUS.md)): webhook delivery, group/nickname/ban synchronisation, "require linked account to play", voice, alerts,
rotating statuses, per-channel formats, `/discord reload` and `resync` (they answer only),
`language` and `debugger`. What Horizon lacks
for the rest is in [GAPS.md](GAPS.md).

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

The bundle pins Horizon `a589a3dd9` (`[compatibility] horizon_revision`; 0.1.0 was made on
`69fab597a`):

```
horizon-mixins apply --workspace <horizon source> --mods <folder with this bundle> --out <stage>
cd <stage> && cargo build --release -p horizon-client --no-default-features
cargo test -p discordsrv-bridge          # unit tests of the mod's crate, in the stage
```

The crate only uses crates in Horizon's `Cargo.lock` (chrono, log, regex, rustls, serde,
serde_json, sha1, base64, toml, tungstenite, ureq, webpki-roots).

## Licence

Derived from DiscordSRV (https://github.com/DiscordSRV/DiscordSRV, GPL-3.0). This port is
licensed under the GNU General Public License v3.0 or later, see [LICENSE](LICENSE) and
[NOTICE](NOTICE). The commands, messages, config keys and the chat relay follow DiscordSRV 1.30.5's
source (`plugin.yml`, `commands/*`, `listeners/DiscordChatListener`, `DiscordSRV.processChatMessage`,
`messages.yml`, `config.yml`, `linking/*`).

Versions up to 0.2.x were written without reading DiscordSRV's source (from its public config files
and Discord's API documentation); 0.3.0 re-checks everything against the source.

## Tested and untested

0.2.0 (the chat side, linking, the new hooks) is written but not yet built into Horizon: only
the crate's unit tests ran (formats, filters, mentions, embeds, routing, linking and its file,
`server.properties`, the gateway dispatch paths). [STATUS.md](STATUS.md) says what to test first.

0.1.0, tested: unit tests (config, filters, prefix, date patterns, packing, command list, presence,
close codes, websocket accept key, log tailing, 429 parsing), and a Horizon dedicated server
with the mod against a local fake Discord (gateway + REST on 127.0.0.1): login with presence
and intents, heartbeats, console lines arriving in batches, a 429 retried, the topic set at start
and at shutdown, an allowed command run by the console, a forbidden one, a bot's and another
channel's message ignored, op 7 Reconnect followed by a Resume, and a clean close at stop.

Untested: the real Discord (TLS to `gateway.discord.gg` and `discord.com`, real rate limits,
real close codes), what DiscordSRV does with forbidden console commands (this port ignores
them silently), and the exact text DiscordSRV puts in `{name}` and in its code blocks.

## Contributing

Pull requests and improvements are always welcome! Open an issue or a PR -- direct pushes to main are disabled, every change goes through review.
