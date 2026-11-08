# Gaps in Horizon for this port

What Horizon (bundle base `a589a3dd9`) lacks for DiscordSRV's behaviour, as one list for an
upstream request. Each line: the hook or helper wanted, the feature, and what the port does today.

## Hooks

- [ ] `native_player_died_v2(&mut self, player: u128, name: &str, message: &Text)` (the
  rendered death message `ServerPlayer.die` broadcasts, `CombatTracker.getDeathMessage()`, also
  when `showDeathMessages` is off) — needed for the death notification's `%deathmessage%` —
  today: `native_player_died`'s key is translated with the victim's name only; a killer's or
  item's name (`death.attack.player`'s `%2$s`) is missing, and a message the game builds from
  the combat (no key) falls back to `death.attack.generic`.
- [ ] `native_player_join_message(&mut self, player: u128, name: &str, line: &mut Option<Text>)`
  and `native_player_leave_message(...)` (the line `PlayerList` broadcasts, Bukkit's
  `PlayerJoinEvent.joinMessage`) — needed for `%message%` of the join/leave notifications as
  other mods changed it, and for silent joins (`discordsrv.silentjoin`/`silentquit`, vanished
  players) — today: vanilla's `multiplayer.player.joined`/`left` translated; every join and leave
  is announced.
- [ ] `native_player_logged_in(..., first: bool)` or a helper `native_has_played_before(uuid) ->
  bool` (Bukkit `hasPlayedBefore`) — needed for `MinecraftPlayerFirstJoinMessage` — today: at
  `native_player_login` the port looks for `<level-name>/players/data/<uuid>.dat` (level name
  read from `server.properties`); wrong if the player directory moves.
- [ ] `native_server_stopped(&mut self, dedicated: bool)` after the players were disconnected
  (or `native_player_logged_out` before `native_server_stopping`) — needed to send the leave
  notifications of a shutdown before the shutdown message, as on Paper — today: the stop glue
  sends a leave notification for each player still listed, then stops the bridge; the later
  `native_player_logged_out` calls find it stopped.

- [x] Permission nodes for `/discord`: 0.3.0 asks `native_has_permission(player, node, default_level)` for every
  `discordsrv.*` node, so a permissions mod decides. Left: the command tree (completion) filters each usage by
  level only; Horizon takes one node per root command, the first usage's (`discordsrv.discord`).
- [ ] `/discord reload` and `/discord resync` need the bridge to re-read its config and rebuild its
  state while threads run (config is shared, not behind a lock); today reload only answers.

## Helpers

- [ ] `native_advancement_display(&self, id: &str) -> Option<AdvancementDisplay { title: Text,
  frame: &str, announce_to_chat: bool, hidden: bool }>` — needed to announce only the
  advancements the game announces, with data pack titles — today: recipes and `*/root` are
  skipped, the title is the vanilla key `advancements.<path with dots>.title`; a data pack's
  advancement is never announced, a vanilla one with `announce_to_chat: false` always is.
- [ ] `native_game_rule(&self, name: &str) -> Option<String>` (server side, no tick context
  needed) — needed for `show_death_messages` and `announce_advancements`, which DiscordSRV obeys
  — today: ignored, notifications go out whatever the rules say.
- [ ] `native_nickname(&self, uuid: u128) -> Option<String>` (the getter to API 62's
  `native_set_nickname`) — needed for `%displayname%` in chat, notifications and the player
  list — today: the player's name.
- [ ] `native_permission(&self, uuid: u128, node: &str) -> Option<bool>` and
  `native_primary_group(&self, uuid: u128) -> Option<String>`, answered by a permissions mod
  (a cross-mod service, as Vault is on Bukkit) — needed for `%primarygroup%`, the
  `discordsrv.*` permissions (link, chat, silent join), and group/role synchronisation — today:
  `%primarygroup%` is empty (the no-group format is used), everyone may link, group sync not
  written.
- [ ] `native_broadcast_quiet(&self, line: Text)` (to every player, not to the log) — needed
  for `DiscordChatChannelBroadcastDiscordMessagesToConsole = false` — today: Discord messages
  are always logged (`native_broadcast` logs every line).
- [ ] `native_player_skin(&self, uuid: u128) -> Option<String>` (the textures property's skin
  hash) — needed for `{texture}` in `AvatarUrl` — today: the UUID without dashes stands in.
- [ ] `native_play_sound_to(&mut self, uuid: u128, sound: &str, volume: f32, pitch: f32) ->
  bool` (server side, one player; only `play_local_sound` exists, client side) — needed for
  `MinecraftMentionSound` (a ping when a Discord message names a player) — today: no sound.
- [ ] `native_server_status(&self) -> (String motd, u32 max_players)` — needed for `%motd%` and
  `%playermax%` — today: read from `server.properties` at start (a changed MOTD needs a restart).

## Crates

- None missing: the port uses chrono, log, regex, rustls, serde, serde_json, sha1, base64, toml,
  tungstenite, ureq and webpki-roots from Horizon's `Cargo.lock`. An emoji name table (for
  `DiscordChatChannelEmojiBehavior = "name"`) would be data, not a crate.

## Text and selectors (0.3.0)

- [ ] A `Text` with a click event `copy_to_clipboard` and a hover text (`ClickToCopyCode` on the link code) —
  today the code is plain chat text.
- [ ] `native_select_entities(selector, executor) -> Vec<String>` (Bukkit's `selectEntities`) — needed to turn
  `@a`/`@p`/`@s` of `/discord broadcast` into names (`PlayerUtil.convertTargetSelectors`) — today left as typed.
- [ ] `native_offline_player(name) -> Option<(u128, String)>` (Bukkit's `getOfflinePlayer`/`getOfflinePlayers`) —
  needed for `/discord link <name> ...` and `unlink <name>` with players who are not on — today `usercache.json`
  beside `server.properties` is read.

## Command tree

- [ ] `/discord ?` is answered by the handler but cannot be in the players' command tree: Horizon's usage parser
  accepts only lowercase letters, digits, `_` and `-` as literal words (`usage_nodes` in
  `horizon-client/src/game/native_server.rs`), so the usage is left out with a warning. Needs a literal kind for
  punctuation, or a fixed help alias.
- [ ] `/discordsrv help` lists the commands as `/discord ...` (DiscordSRV does the same: its help text is fixed).
