# hmod port: discordsrv
Parity: 100% of the rec100 item list (27/27 items, round 6; the 4 n/a that stay are java-only/covered). Compiled by the central queue as r6-discordsrv-1.
Round 4 (2026-11-25, blind, not compiled): `alerts.rs` (alerts.yml), `requirelink.rs` (the login checks), reserializer in `format.rs`, the proxy (`config::Proxy`, `rest::with_proxy`, `ws::tunnel`). The `next-engine` feature is gone (`host.translate` is normal code). New unit tests cover the expression language, the check order, markdown both ways and a CONNECT tunnel on a local socket.

Method: 10 commands + 21 nodes + 167 config keys + 17 features = 215 items; blocked-by-API counts as not done. Round 4 counted +5 webhook keys, +2 webhook features (chat, notices), +4 linking keys (enabled, bypass names, not linked message, still starting).
Round 1
- Fixed:
  - `src/lib.rs` `console` tick: `host.server_run_command_captured` replaced by `host.commands().run_command_with_output` (`CommandResult.success` / `.feedback`).
  - `src/lib.rs` `has_node`: now `&mut dyn ServerHost`, asks `permission_bool(...).unwrap_or(false)` for the typed node. Offline players keep the old rule (only level-0 nodes).
  - `src/lib.rs` `player_command`: the bridge's `has` closure is a plain `Fn`, so the answers for all declared nodes are read once per `/discord` command into a `BTreeMap`, then looked up. Undeclared names are denied.
  - `src/lib.rs` `player_logged_in` / `player_logged_out`: `has_node(&mut *host, ...)`.
  - `src/lib.rs` `permission_gather_nodes` (new hook) declares `NODES`; `register_commands` uses `with_node(&node("discordsrv.discord"))`.
  - Imports: permission types, `NodeDefault`, `PermissionGatherNodesEvent`, `BTreeMap`.
- Permission nodes (declared from `NODES` in `src/lib.rs`):
  - Level 2: `discordsrv.bcast`, `discordsrv.debug`, `discordsrv.language`, `discordsrv.reload`, `discordsrv.resync`, `discordsrv.unlink`, `discordsrv.link.others`, `discordsrv.linked.others`, `discordsrv.silentjoin`, `discordsrv.silentquit`, `discordsrv.discord`.
  - Level 0: `discordsrv.help`, `discordsrv.link`, `discordsrv.linked`.
- Gaps: none.
- Unsure:
  - `discordsrv.discord` is declared at Level 2, because the staff check asks it with default 2. It is also the `with_node` of every usage, including the level-0 ones (`help`, `link`, `linked`). With the built-in handler the usage level is used, so this has no effect. With a custom handler, the single node gates those usages too, unlike the old level fallback. Needs a decision when a handler is selected.
  - The node answers are computed for all 14 nodes on each `/discord` command, not only the ones the bridge asks. Harmless, but it is more queries than before.
  - Built-in handler, online player: a level-0 node is granted via its default (level 0), the same as the old "not explicitly denied" rule.

Round 2 (parity fixes)
- Fixed: `silentjoin`/`silentquit` default no longer operator (level 5, the highest); `silentquit` is read at login and used at logout and shutdown; `discordsrv.discord` defaults to everyone and each usage has its own node; `discordsrv.chat` (default everyone) gates chat relay; declared `unlink.others`, `nicknamesync`, `updatenotification`, `groupsyncwithcommands`; `/discord` now works from the console, RCON and command blocks (`run_mod_command`, console sender id `bridge::CONSOLE`, console answers for `link`, `linked`, `unlink` without arguments as in the original); the bridge-off permission line uses the sub-command's own node.
- Gaps: `/discord reload` (config swap needs a bridge restart), `resync`, `debug(ger)`, `?` usage, `MinecraftMentionSound` and other unread config keys, parent nodes `discordsrv.player`/`discordsrv.admin`. Level-4 operators still hold the silent nodes (no "nobody" default).

Round 3 (parity, written blind, not compiled)
- Added: `/discord reload` (`bridge::reload`: old threads end quietly, config read again, new bridge started, also from "off"), `debugger.rs` (`/discord debugger` start/stop/upload to a local file, `Debug` list, two debug log points), catch-all usage `<sub:word> [args:text]` so `?` reaches the bridge, `MinecraftMentionSound` (`playsound ... pling`), keys `ForcedLanguage`, `Timezone`, `Debug`. `resync` keeps the original disabled-sync line.
- Dropped finding: `/discord debug` is only usage text in plugin.yml; the original answers "doesn't exist".
- Gaps (API): see GAPS_UPSTREAM.md (nobody default, parent nodes, advancement title, zones). Not done: webhooks.

Round 4 (100 % brief, blind, not compiled)
- Silent join and quit: `NODES` uses `NOBODY`; `node_default` declares `NodeDefault::Nobody` behind the new cargo feature `horizon_perm_nobody` (off; engine PR 18754), otherwise level 5.
- Death notices: `vanilla_line` has the en_us lines of 20 death keys (19 causes: lava, drown, fall, starve, cactus, explosion, onFire, inFire, inWall, outOfWorld, magic, wither, anvil, fallingBlock, dragonBreath, freeze, sweetBerryBush, stalagmite, lightningBolt; plus generic). Unit tests in `src/lib.rs`.
- Webhook delivery (`Experiment_WebhookChatMessage*`): `rest.rs` finds or makes the bot's webhook named `DiscordSRV` per channel (kept for the run), `execute_webhook` posts to it; errors use the label `POST webhook`, never the URL (the token is in the path). `chat::webhook_message` mirrors `WebhookUtil`: game filters, format, `[` escape, username format, `%discordname%`/`%discordusername%`, username from Discord, 80 characters. Avatar from Discord from the member cache (`Member::avatar_url`). Own webhook messages are never echoed back.
- Notification `Webhook` sections (`Enable`/`Enabled`, `Name`, `AvatarUrl`): sent through the webhook with the notice's content and embeds.
- Require linked account to play (`linking.yml`): `player_login` refuses an unlinked player with the kick text (`{BOT}`, `{CODE}`, `{INVITE}`, `&` codes); bypass names; the still-starting text. The importer flags the section when it is on (the old check read a bool from a map and never fired).
- Not done and why: membership check, subscriber roles, vanilla whitelist and ban checks (no safe REST call on the login thread; no whitelist/ban API), killer and item in death lines (`player_died` has only the cause key), advancement titles, ban sync, group sync, nickname sync, `/discord resync` (APIs missing, see GAPS_UPSTREAM.md), Experiment_Jdbc/MCDiscordReserializer, proxy.

Checklist round 100 (walked against plugin.yml, commands/*, listeners/*, config en.yml, messages en.yml, linking en.yml, WebhookUtil, DiscordSRV.java)
Commands
- [x] `/discord` (invite), `help`, `?` (catch-all word), `link`, `linked`, `unlink`, `clearlinked`, `broadcast`, `bcast`, `reload`, `language`/`setlanguage`/`lang`/`setlang`, `debugger`, alias `discordsrv`
- [x] `debug`: not a command in DiscordSRV 1.30 (usage text only): "That command doesn't exist!"
- [~] `resync`: answers only (group sync API)
Permission nodes (21: 2 parents + 19 children)
- [x] 19 children declared with their defaults (`discordsrv.chat`, `.discord`, `.help`, `.link`, `.linked`, `.bcast`, `.reload`, `.debug`, `.link.others`, `.linked.others`, `.unlink`, `.unlink.others`, `.resync`, `.language`, `.updatenotification`, `.groupsyncwithcommands`, `.nicknamesync`, `.silentjoin`, `.silentquit`)
- [~] `silentjoin`/`silentquit`: nobody default only with `horizon_perm_nobody`
- [~] `nicknamesync`, `groupsyncwithcommands`, `resync`: declared, no behaviour (sync APIs)
- [~] `updatenotification`: declared, no behaviour (no update check on Horizon)
- [ ] `discordsrv.player`, `discordsrv.admin` (parents: no children API)
Config (en.yml keys)
- [x] channels, console channel keys, chat channel keys, notifications, linking keys, watchdog, topic and channel updaters, canned responses, game status, timestamp/date format, MinecraftMentionSound, Debug, AvatarUrl, CancelConsoleCommandIfLoggingFailed, StatusUpdateRateInMinutes, DiscordInviteLink
- [x] Experiment_WebhookChatMessageDelivery / UsernameFormat / Format / UsernameFromDiscord / AvatarFromDiscord
- [~] Experiment_WebhookChatMessageUsernameFilters: the original loads it and never applies it; not read here
- [x] Require linked account to play: Enabled, Bypass names, Not linked message, Messages/DiscordSRV still starting
- [~] ForcedLanguage, Timezone: English / server zone only
- [ ] Experiment_Jdbc*, Experiment_MCDiscordReserializer_*, ProxyHost/Port/User/Password
- [n/a] DisabledPluginHooks, VentureChatBungee, UseModernPaperChatEvent, NoopHostnameVerifier, ForceTLSv, DNS key, EnablePresenceInformation, ConfigVersion
Messages (messages.yml, English only)
- [x] join, first join, leave, startup, shutdown, watchdog, chat both ways, player list, linking answers, command answers, Webhook sections
- [~] death: 19 causes have their line (plus generic); killer/item causes use the generic "died"
- [ ] advancement announcements (title not readable)
Listeners
- [x] PlayerJoinLeave (incl. silent), PlayerChat (discordsrv.chat, webhook), DiscordConsole, DiscordAccountLink, watchdog, server stop leave notices
- [~] PlayerDeath
- [ ] PlayerAdvancementDone / PlayerAchievements, PlayerBanListener / DiscordBanListener
- [n/a] ModernPlayerChatListener (Paper AsyncChatEvent; server_chat is used), DiscordDisconnectListener (logs only)
Features
- [ ] voice.yml, alerts.yml, group/nickname/ban synchronisation, linking membership/subscriber/whitelist/ban checks, "Kicked for unlinking"

## Round 100b (for coreconsole)
- `Preprocess::Refuse` posts its reply to the console channel at once (a `Job::Post`), not with the next log batch.
- `add_console_preprocessor(f) -> PreprocessorHandle` and `remove_console_preprocessor(handle)`: several preprocessors chain in order, the first `Refuse` ends it; `set_console_preprocessor` still replaces all. Test `several_preprocessors_chain_and_one_can_be_removed`.
