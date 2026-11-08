# Status

**0.3.0 (source-checked, still blind):** the licence allows reading DiscordSRV's source (GPL-3.0), so commands,
messages, config keys, the Discord-to-game and game-to-Discord chat paths, the link channel, the usage log and the
console command filter were compared with it. Nothing was compiled or run (no cargo in this pass): build and run
`cargo test -p discordsrv-bridge` first. See the end of this file for what still differs.

Blind port (2026-11-06): the core crate builds and its unit tests pass (`cargo test` in
`bridge/`); the glue and the bundle have not been through `horizon-mixins apply` or an engine
build on `a589a3dd9` yet. Version 0.1.0 (console channel) was proven on a Horizon server; 0.2.0
adds the rest blind.

The servers this was made for run DiscordSRV with the console channel set and **`Channels`
empty**, so every chat channel feature below is dormant there until a channel is set, and the
group sync's role IDs are placeholders (off). The port does the same: no `Channels`, no chat.

## Features

| Feature (DiscordSRV) | State | Notes |
|---|---|---|
| Console channel: log forwarding, filters, prefix, code blocks | written | 0.1.0, proven on Horizon |
| Console channel: commands, black/whitelist, usage log | written | 0.1.0, proven on Horizon |
| Console channel topic (start, timer, shutdown) | written | 0.1.0, proven on Horizon |
| Bot presence (`DiscordGameStatus`, `DiscordOnlineStatus`) | partly | first entry only; no rotation, no `%online%` |
| `Channels` map, routing by kind (global/status/awards/deaths/join/leave/watchdog/link) | written | ordered pairs instead of a YAML map |
| Chat Minecraft -> Discord (format, no-group format, prefix rule, game filters, escaping) | written | `%primarygroup%` always empty, `%displayname%` = name (GAPS) |
| Mention translation Minecraft -> Discord (`@user`, `#channel`, `@role`) | partly | only members seen in messages (no members intent) |
| `DiscordChatChannelAllowedMentions` | written | |
| Chat Discord -> Minecraft (formats, roles, aliases, colour, reply, truncation) | written | |
| Blocked bots/webhooks/IDs/roles, roles selection | written | |
| Colour codes for listed roles | written | |
| Discord filters | written | |
| Emote behaviour (`name`/`hide`) | written | |
| Emoji behaviour | partly | `hide` and `show`; `name` keeps the emoji (no name table) |
| Attachments shown as URLs | written | 0.3.0: verified: each attachment (3 at most) and sticker is its own line through the chat format |
| `DiscordChatChannelRequireLinkedAccount` | written | PM with `DiscordLinkedAccountRequired` |
| `DiscordChatChannelBroadcastDiscordMessagesToConsole = false` | not started | GAPS: broadcasts are always logged |
| Per-channel formats (`DiscordToMinecraftChatMessageFormat_<channel>`) | not started | |
| Player list command (with expiry and deleting the request) | written | 0.3.0: DiscordSRV's layout (`n/max`, code block, sorted, cut at 1996) |
| Canned responses | written | 0.3.0: trigger = message starts with it (any case), any channel, message not forwarded, colour codes stripped |
| `!c` console commands from a chat channel | written | 0.3.0: roles, whitelist/blacklist, bypass roles, usage log, answer back, expiry, notify DM (`DiscordChatChannelConsoleCommand*`) |
| Join / first join / leave notifications (embeds) | written | `%message%` is vanilla's line (GAPS); first join by player file |
| Death notification | partly | message from the death key + victim name only (GAPS) |
| Advancement notification | partly | vanilla titles by key, no `announce_to_chat` check (GAPS) |
| Webhook delivery of notifications/chat | not started | off in the reference setup (`Webhook.Enable: false`) |
| Startup / shutdown messages | written | startup after the first tick; leaves first at shutdown |
| Chat channel topic (timer, shutdown) | written | |
| `ChannelUpdater` (channel names) | written | placeholder IDs in the reference setup: dormant |
| Server watchdog | written | own thread, `%guildowner%` from GUILD_CREATE |
| Account linking: `/discord link`, code by PM, `linked`, `unlink` | written | |
| Linking in a `link` channel (`UsePM = false`) | written | message delete seconds not ported |
| Linked role, linked/unlinked console commands | written | |
| `accounts.aof` storage | written | 0.3.0: reads DiscordSRV's append-only file (`<id> <uuid>` and `-<id>`/`-<uuid>` lines) |
| Relink by a new code | written | |
| `/discord` (invite), `/discordsrv` alias | written | the invite text is the `DiscordCommandFormat` default (checked against docs only) |
| `/discord help [command]`, `?` | written | 0.3.0: source text and layout (random title/command colours, per-permission); `?` answers but is not in the command tree (GAPS.md) |
| `/discord clearlinked` (alias of `unlink`) | written | |
| `/discord broadcast` (`bcast`) | written | 0.3.0: `#channel` by ID or name, else the `broadcasts` channel (first channel if none); `\n` newlines, mentions translated; no reply on success; "No text given to broadcast"; `@a/@p` selectors not converted (GAPS.md) |
| `/discord link|linked|unlink <target>` (others) | written | 0.3.0: DiscordSRV's target resolution (UUID, Discord ID, Discord name, Minecraft name) and its "Interpreted target as" output; offline Minecraft names are found only through `usercache.json` or online players |
| `/discord unlink` for one's own account | written | `discordsrv.unlink` defaults to op (plugin.yml); others ask `discordsrv.linked.others` as DiscordSRV does |
| `/discord resync` | written | answers the "Group synchonization requires valid ..." line (group sync is not ported) |
| `/discord reload` | partly | still only says a restart is needed (GAPS.md) |
| `/discord language`, `lang`, `setlang`, `setlanguage` | partly | asks before changing; only English exists, other languages answer "does not have a translation"; `debugger` says it is not part of the port |
| Require linked account to play (`linking.yml`) | not started | off in the reference setup |
| Group/role synchronisation | not started | off in the reference setup (placeholder role IDs); needs a permissions API (GAPS) |
| Nickname synchronisation | not started | off in the reference setup |
| Ban synchronisation | not started | off in the reference setup |
| Voice | not started | off in the reference setup |
| Alerts | not started | none configured |
| Dynmap / chat plugin hooks / PlaceholderAPI | not started | no such mods on Horizon |
| JDBC account backend | not started | experimental option, not used |

Counts: written 29, partly 8, not started 13 (the latest pass added help, `?`, `clearlinked` and the
`discordsrv` alias, and split the staff subcommands out of "other").

Permissions (DiscordSRV plugin.yml): `discordsrv.discord`, `.help`, `.link`, `.linked` default to
everyone. `discordsrv.unlink` and the staff nodes (`.bcast`, `.link.others`, `.linked.others`,
`.reload`, `.resync`, `.debug`, `.language`) default to op. 0.3.0 asks every node through
`native_has_permission(player, node, default_level)` (commands.rs), so a permissions mod can grant or
refuse them; the command tree (completion) still filters by level only.

## Test first, once a build is possible

1. `horizon-mixins apply` on `a589a3dd9`: the new injections (`player_login`,
   `player_logged_in/out`, `player_died`, `player_advancement_done`, `player_chat`,
   `player_command`, `register_commands`) and their binding types (`Option<&str>`,
   `&mut Vec<horizon_client::text::Text>`, `horizon_client::mod_api::NativeCommand`), and
   `Text::legacy`/`Text::translate` being reachable from the glue.
2. Console-only config (no `Channels`): behaves exactly as 0.1.0 (the wave-1 fake-Discord test
   again), now with the DIRECT_MESSAGES intent in Identify.
3. With a chat channel against the local fake Discord: game chat out, a Discord message in
   game, the bot's own message not echoed, join/leave/death/advancement embeds, startup after
   the first tick, at stop: leaves, shutdown message, shutdown topic.
4. `/discord link` -> PM code -> linked role PUT, in-game confirmation, `accounts.aof` written;
   `/discord unlink`.
5. Whether `native_server_players` answers inside `native_player_died` (the glue falls back to
   the bridge's last snapshot).
6. Staff commands as an operator and as a player: `/discord link <player> <discord>` (tells the player,
   Linked job), `/discord unlink <player>`, `/discord bcast #<id> text`; non-operators get the
   NoPermissionMessage for them and for `/discord unlink`.
7. Watchdog: stall the server thread past `ServerWatchdogTimeout` (e.g. stop the process with
   SIGSTOP briefly is not enough: the bridge threads stop too; a debugger break on the server
   thread only).

## 0.3.0: what was changed against the source, and what still differs

Changed (source file in DiscordSRV 1.30.5): `/discord` sub-commands, aliases, permission nodes and replies
(`commands/*`, `CommandManager`, `plugin.yml`); help layout (`CommandHelp`); account file reading and code
parsing (`AppendOnlyFileAccountLinkManager`, `AbstractFileAccountLinkManager.process`); the link channel is
processed whatever `UsePM` says, with `MinecraftDiscordAccountLinkedMessageDeleteSeconds` and replies
(`DiscordAccountLinkListener`); Discord to game: webhook block first, canned responses, list command, `!c`, linked
account requirement (deletes the message, PM cut to fit), blocked roles (cross reaction), truncation (reactions),
roles selection/colour-code roles by name or ID, aliases by ID then lower-case name, JDA 4 effective names
(nickname or username, no global display names), mentions that the bot does not know vanish (`DiscordChatListener`,
`DiscordSRVMinecraftRenderer`); game to Discord: the prefix rule with an empty prefix, `@` defused when mention
translation is off, `@role`/`@member` only (no `#channel`), colour codes stripped from the finished pattern, game
filters on the finished text, `%channelname%`/`%time%` (`DiscordSRV.processChatMessage`, `DiscordUtil`); console
channel: the first word without `namespace:` is looked up in the list, the usage log line and file name
(`DiscordConsoleListener`, `TimeUtil`).

Still differs:

- `/discord reload`: only a message (config is read once).
- `%primarygroup%`, `%displayname%` (GAPS.md), per-channel formats (`DiscordToMinecraftChatMessageFormat_<channel>`),
  webhook delivery, `Experiment_MCDiscordReserializer_*` (Discord markdown to Minecraft styles and back),
  `BroadcastDiscordMessagesToConsole = false`.
- The member list is what the bot saw in messages (no members intent): name lookups for `link`/`linked`/`unlink`
  and `@name` mentions only know those; DiscordSRV knows the whole guild.
- `/discord link <player> <user>` needs the player online or in `usercache.json`; DiscordSRV links any name it is
  given to an offline UUID.
- `/discord broadcast`: `@a`/`@p` selectors are not turned into names; `:emote:` names are not turned into emote
  mentions; DiscordSRV splits an overlong message, this port cuts it at 2000.
- The "Click to copy" hover/click on the link code needs Text events from Horizon (GAPS.md).
- Role colour for a role without colour: DiscordSRV's value for it is not in the repository's source (JDA); the
  port uses no colour code.
