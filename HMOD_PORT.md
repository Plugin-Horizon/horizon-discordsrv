# hmod port: discordsrv
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
