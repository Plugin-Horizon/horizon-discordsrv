//! DiscordSRV's voice module (voice.yml, `modules/voice/VoiceModule` and `Network`): players who are close
//! to each other in the game are put into a shared Discord voice channel (a "network"), everyone else waits
//! in the lobby channel. The decisions are made by [`Voice::tick`] from plain data (the players' positions,
//! the links, the gateway's voice states), so they run without a server or a Discord; the REST calls
//! that follow are [`Action`]s, performed by [`run`] on the voice thread.
//!
//! As in the original: a player is "dirty" when they joined or moved, only dirty players are looked at, a
//! network starts when two linked players in the category are within `Horizontal Strength` and
//! `Vertical Strength`, a player joins a network when in range of one of its players and leaves it
//! beyond the strength plus `Falloff`, networks that touch merge, a network with no players is deleted
//! when nobody sits in its channel, and (when asked) people who can speak in the lobby are server-muted
//! there. The voice states need the `GUILD_VOICE_STATES` intent, which the gateway adds when voice is on.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::discordsrv_bridge::Shared;
use crate::discordsrv_bridge::config::VoiceSettings;
use crate::discordsrv_bridge::guild::{Cache, ChannelInfo, perm};
use crate::discordsrv_bridge::linking::Accounts;

/// How long a move that was asked for is not asked again (the gateway's voice state takes a moment).
const MOVE_GRACE: Duration = Duration::from_secs(3);
/// Discord allows 50 channels in a category.
const CATEGORY_LIMIT: usize = 50;
/// A missing permission is logged at most this often.
const LOG_EVERY: Duration = Duration::from_secs(30);

/// A player as the server thread hands them over for the voice module.
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePlayer {
    /// The dimension (the original compares world names, which are one-to-one with dimensions).
    pub world: String,
    pub pos: [f64; 3],
    /// Not dead (`!Player#isDead`).
    pub alive: bool,
}

/// A group of players who hear each other, with their voice channel.
#[derive(Debug, Clone)]
pub struct Network {
    pub id: u64,
    pub players: HashSet<u128>,
    /// The Discord voice channel; `None` until it is created.
    pub channel: Option<String>,
    /// The channel was created by this run (a channel found at start only waits to be emptied and deleted).
    pub initialized: bool,
}

/// What the voice thread does on Discord after a tick.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Create the voice channel of network `network`.
    CreateNetwork { network: u64, name: String, overwrites: Value },
    Move { user: String, channel: String },
    Delete { channel: String },
    Mute { user: String, mute: bool },
    /// Set the lobby's @everyone overwrite (it must deny speaking).
    LobbyOverride { allow: u64, deny: u64 },
}

/// What one tick decided.
#[derive(Debug, Default)]
pub struct Tick {
    pub actions: Vec<Action>,
    /// Lines for the error log (a missing permission).
    pub errors: Vec<String>,
    /// The bot's roles are not known yet: fetch them for this guild.
    pub fetch_bot_roles: Option<String>,
}

#[derive(Debug, Default)]
pub struct Voice {
    pub networks: Vec<Network>,
    next_id: u64,
    players: HashMap<u128, VoicePlayer>,
    dirty: HashSet<u128>,
    /// Users this module server-muted in the lobby.
    muted: HashSet<String>,
    recent: HashMap<String, (String, Instant)>,
    last_log: Option<Instant>,
    override_sent: Option<Instant>,
    scanned: bool,
    fetch_after: Option<Instant>,
    /// Actions the gateway events asked for (mutes), taken by the next tick.
    pending: Vec<Action>,
}

/// A channel name that is a UUID: how the original recognises the channels of its networks.
pub fn is_uuid_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// A random version 4 UUID, as the name of a new network's channel (`UUID.randomUUID()`).
fn random_uuid(seed: u64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let word = |extra: u64| {
        let mut h = state.build_hasher();
        h.write_u64(seed);
        h.write_u64(extra);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.finish()
    };
    let (a, b) = (word(1), word(2));
    let n = ((a as u128) << 64 | b as u128) & !(0xF000u128 << 64) | (0x4000u128 << 64);
    let n = n & !(0xC000_0000_0000_0000u128) | 0x8000_0000_0000_0000u128;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (n >> 96) as u32,
        (n >> 80) as u16,
        (n >> 64) as u16,
        (n >> 48) as u16,
        n & 0xFFFF_FFFF_FFFF
    )
}

fn horizontal(a: &VoicePlayer, b: &VoicePlayer) -> f64 {
    ((a.pos[0] - b.pos[0]).powi(2) + (a.pos[2] - b.pos[2]).powi(2)).sqrt()
}

fn vertical(a: &VoicePlayer, b: &VoicePlayer) -> f64 {
    (a.pos[1] - b.pos[1]).abs()
}

/// Same world and within the strengths plus `extra` (the falloff).
fn near(cfg: &VoiceSettings, a: &VoicePlayer, b: &VoicePlayer, extra: f64) -> bool {
    a.world == b.world
        && vertical(a, b) <= cfg.network.vertical_strength + extra
        && horizontal(a, b) <= cfg.network.horizontal_strength + extra
}

fn block(p: &VoicePlayer) -> (String, i64, i64, i64) {
    (p.world.clone(), p.pos[0].floor() as i64, p.pos[1].floor() as i64, p.pos[2].floor() as i64)
}

impl Voice {
    /// The server's players now. A player who is new, or has moved a block or changed world, is dirty
    /// (the original marks them on join, move and teleport); one who is gone leaves their networks.
    pub fn update_players(&mut self, list: Vec<(u128, VoicePlayer)>) {
        let mut now = HashMap::new();
        for (uuid, p) in list {
            if self.players.get(&uuid).is_none_or(|old| block(old) != block(&p)) {
                self.dirty.insert(uuid);
            }
            now.insert(uuid, p);
        }
        let gone: Vec<u128> = self.players.keys().filter(|u| !now.contains_key(u)).copied().collect();
        for uuid in gone {
            self.remove_player(uuid);
            self.dirty.remove(&uuid);
        }
        self.players = now;
    }

    fn remove_player(&mut self, uuid: u128) {
        for n in &mut self.networks {
            n.players.remove(&uuid);
        }
    }

    /// The network that holds `uuid`.
    pub fn network_of(&self, uuid: u128) -> Option<&Network> {
        self.networks.iter().find(|n| n.players.contains(&uuid))
    }

    /// A channel was deleted in Discord: its network is gone (`onVoiceChannelDelete`).
    pub fn channel_deleted(&mut self, channel: &str) {
        self.networks.retain(|n| n.channel.as_deref() != Some(channel));
    }

    /// The channel of a network the voice thread made.
    pub fn set_channel(&mut self, network: u64, channel: &str) {
        if let Some(n) = self.networks.iter_mut().find(|n| n.id == network) {
            n.channel = Some(channel.to_string());
            n.initialized = true;
        }
    }

    /// A network whose channel could not be made is dropped again.
    pub fn drop_network(&mut self, network: u64) {
        self.networks.retain(|n| n.id != network);
    }

    /// The guild of the voice category.
    pub fn guild_id<'a>(cache: &'a Cache, cfg: &VoiceSettings) -> Option<&'a str> {
        cache.channels.get(&cfg.category).map(|(_, g)| g.as_str())
    }

    /// A voice state changed (`user` went from `old` to `new`; either may be none): the original's
    /// `onGuildVoiceJoin`, `Move` and `Leave`.
    pub fn on_voice_change(
        &mut self,
        cfg: &VoiceSettings,
        cache: &Cache,
        accounts: &Accounts,
        user: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        if let Some(channel) = new
            && let Some(action) = self.check_muted(cfg, cache, channel, user)
        {
            self.pending.push(action);
        }
        let linked = accounts.uuid_of(user).filter(|u| self.players.contains_key(u));
        if old.is_none()
            && new == Some(cfg.lobby.as_str())
            && let Some(uuid) = linked
        {
            self.dirty.insert(uuid);
        }
        if let (Some(old), Some(uuid)) = (old, linked)
            && cache.parent_of(old) == Some(cfg.category.as_str())
        {
            let left_category = match new {
                None => true,
                Some(n) => cache.parent_of(n).is_some_and(|p| p != cfg.category),
            };
            if left_category {
                self.remove_player(uuid);
            }
        }
    }

    /// `checkMutedUser`: someone who can speak in the lobby although the lobby denies it is server-muted
    /// (when `Mute users who bypass speak permissions in the lobby` is on); anyone this module muted
    /// is unmuted when they leave the lobby.
    fn check_muted(&mut self, cfg: &VoiceSettings, cache: &Cache, channel: &str, user: &str) -> Option<Action> {
        let guild = cache.guilds.get(Self::guild_id(cache, cfg)?)?;
        let is_lobby = channel == cfg.lobby;
        if is_lobby && !cache.guild_muted.contains(user) {
            if !cfg.mute_bypassers {
                return None;
            }
            let info = cache.channel_info.get(channel)?;
            let denied = info.overwrites.iter().find(|o| o.kind == 0 && o.id == guild.id).is_some_and(|o| o.deny & perm::SPEAK != 0);
            if !denied {
                return None;
            }
            let roles = cache.roles_of(user).unwrap_or(&[]);
            let held = cache.permissions_in(guild, user, roles, channel);
            let bot_roles = cache.roles_of(&cache.bot_id)?;
            let bot_in_lobby = cache.permissions_in(guild, &cache.bot_id, bot_roles, channel);
            let bot_in_category = cache.permissions_in(guild, &cache.bot_id, bot_roles, &cfg.category);
            if held & perm::SPEAK != 0
                && held & perm::MUTE_MEMBERS != 0
                && bot_in_lobby & perm::MUTE_MEMBERS != 0
                && bot_in_category & perm::MOVE_MEMBERS != 0
            {
                self.muted.insert(user.to_string());
                return Some(Action::Mute { user: user.to_string(), mute: true });
            }
        } else if !is_lobby && self.muted.remove(user) {
            return Some(Action::Mute { user: user.to_string(), mute: false });
        }
        None
    }

    fn in_range_to_be_added(&self, cfg: &VoiceSettings, net: usize, uuid: u128) -> bool {
        let Some(me) = self.players.get(&uuid) else { return false };
        self.networks[net].players.iter().filter(|u| **u != uuid).filter_map(|u| self.players.get(u)).any(|p| near(cfg, p, me, 0.0))
    }

    fn in_range_to_stay(&self, cfg: &VoiceSettings, net: usize, uuid: u128) -> bool {
        let Some(me) = self.players.get(&uuid) else { return false };
        self.networks[net]
            .players
            .iter()
            .filter(|u| **u != uuid)
            .filter_map(|u| self.players.get(u))
            .any(|p| near(cfg, p, me, cfg.network.falloff))
    }

    /// `network1.size() > network2.size() ? network1.engulf(network2) : network2.engulf(network1)`: the index of the winner.
    fn engulf_pair(&mut self, a: usize, b: usize) -> usize {
        let (winner, loser) = if self.networks[a].players.len() > self.networks[b].players.len() { (a, b) } else { (b, a) };
        let taken: Vec<u128> = self.networks[loser].players.drain().collect();
        self.networks[winner].players.extend(taken);
        winner
    }

    /// The overwrites of a new network's channel: @everyone may speak, may not connect (and sees it only
    /// when `Channels are visible`); the bot sees, connects and moves.
    fn network_overwrites(cfg: &VoiceSettings, guild: &str, bot: &str) -> Value {
        let mut allow = perm::SPEAK;
        let mut deny = perm::CONNECT;
        if cfg.network.channels_visible {
            allow |= perm::VIEW_CHANNEL;
        } else {
            deny |= perm::VIEW_CHANNEL;
        }
        json!([
            { "id": guild, "type": 0, "allow": allow.to_string(), "deny": deny.to_string() },
            { "id": bot, "type": 1, "allow": (perm::VIEW_CHANNEL | perm::CONNECT | perm::MOVE_MEMBERS).to_string(), "deny": "0" },
        ])
    }

    /// One pass of `VoiceModule#tick`.
    pub fn tick(&mut self, cfg: &VoiceSettings, cache: &Cache, accounts: &Accounts, now: Instant) -> Tick {
        let mut out = Tick::default();
        let Some(guild_id) = Self::guild_id(cache, cfg) else { return out };
        if !cache.channel_info.contains_key(&cfg.lobby) {
            return out;
        }
        let Some(guild) = cache.guilds.get(guild_id) else { return out };
        let Some(bot_roles) = cache.roles_of(&cache.bot_id) else {
            // Asked again after half a minute at the earliest, so a bot that is not in the guild costs nothing.
            if self.fetch_after.is_none_or(|t| now >= t) {
                self.fetch_after = Some(now + LOG_EVERY);
                out.fetch_bot_roles = Some(guild_id.to_string());
            }
            return out;
        };
        if !self.scanned {
            // Channels a run before left in the category are networks without players: emptied, then deleted.
            self.scanned = true;
            let old: Vec<String> = cache
                .channel_info
                .iter()
                .filter(|(id, i)| i.kind == 2 && i.parent.as_deref() == Some(cfg.category.as_str()) && **id != cfg.lobby)
                .filter(|(id, _)| cache.channel_name(id).is_some_and(is_uuid_name))
                .map(|(id, _)| id.clone())
                .collect();
            for channel in old {
                self.next_id += 1;
                self.networks.push(Network { id: self.next_id, players: HashSet::new(), channel: Some(channel), initialized: false });
            }
        }
        out.actions.append(&mut self.pending);

        // The bot needs these permissions, or it could do nothing and would only throw errors.
        let bot = cache.bot_id.as_str();
        let in_lobby = cache.permissions_in(guild, bot, bot_roles, &cfg.lobby);
        let in_category = cache.permissions_in(guild, bot, bot_roles, &cfg.category);
        let log = self.last_log.is_none_or(|t| now.duration_since(t) >= LOG_EVERY);
        let mut stop = false;
        for (bit, name) in [(perm::VIEW_CHANNEL, "View Channel"), (perm::MOVE_MEMBERS, "Move Members")] {
            if in_lobby & bit == 0 {
                if log {
                    out.errors.push(format!("The bot doesn't have the \"{name}\" permission in the voice lobby"));
                }
                stop = true;
            }
        }
        for (bit, name) in [
            (perm::VIEW_CHANNEL, "View Channel"),
            (perm::MOVE_MEMBERS, "Move Members"),
            (perm::MANAGE_ROLES, "Manage Permissions"),
            (perm::MANAGE_CHANNELS, "Manage Channels"),
        ] {
            if in_category & bit == 0 {
                if log {
                    out.errors.push(format!("The bot doesn't have the \"{name}\" permission in the voice category"));
                }
                stop = true;
            }
        }
        if stop {
            // The original stamps the log time on every stopped tick, so it logs once and never again;
            // here the stamp is taken only when a line was written, which is what the 30 seconds mean.
            if log {
                self.last_log = Some(now);
            }
            return out;
        }

        // The lobby must deny speaking to @everyone.
        let public = cache.channel_info.get(&cfg.lobby).and_then(|i| i.overwrites.iter().find(|o| o.kind == 0 && o.id == guild.id));
        let wanted = match public {
            None => Some((0, perm::SPEAK)),
            Some(o) if o.deny & perm::SPEAK == 0 => Some((o.allow & !perm::SPEAK, o.deny | perm::SPEAK)),
            Some(_) => None,
        };
        if let Some((allow, deny)) = wanted
            && self.override_sent.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5))
        {
            self.override_sent = Some(now);
            out.actions.push(Action::LobbyOverride { allow, deny });
        }

        // Networks that have no voice channel any more.
        self.networks.retain(|n| !(n.initialized && n.channel.as_ref().is_none_or(|c| !cache.channels.contains_key(c))));

        let dirty: Vec<u128> = self.dirty.drain().collect();
        for uuid in dirty {
            let Some(me) = self.players.get(&uuid).cloned() else { continue };
            let Some(user) = accounts.discord_of(uuid) else { continue };
            let Some(channel) = cache.voice_states.get(user) else { continue };
            let is_lobby = *channel == cfg.lobby;
            if !is_lobby && cache.parent_of(channel) != Some(cfg.category.as_str()) {
                // They are in some other channel: a move that was asked for is dropped.
                self.recent.remove(user);
                continue;
            }

            // Into the networks they came in range of; networks they bridge become one.
            let candidates: Vec<usize> = (0..self.networks.len()).filter(|&i| self.in_range_to_be_added(cfg, i, uuid)).collect();
            let mut merged: Option<usize> = None;
            for i in candidates {
                merged = Some(match merged {
                    None => i,
                    Some(m) => self.engulf_pair(m, i),
                });
            }
            if let Some(m) = merged {
                self.networks[m].players.insert(uuid);
            }

            // Out of the networks they lost.
            let lost: Vec<usize> =
                (0..self.networks.len()).filter(|&i| self.networks[i].players.contains(&uuid) && !self.in_range_to_stay(cfg, i, uuid)).collect();
            for i in lost {
                self.networks[i].players.remove(&uuid);
                if self.networks[i].players.len() == 1 {
                    self.networks[i].players.clear();
                }
            }

            // A new network when linked players in the category are within the activation distance.
            let within: HashSet<u128> = self
                .players
                .iter()
                .filter(|(u, p)| p.alive && **u != uuid && near(cfg, p, &me, 0.0))
                .filter(|(u, _)| !self.networks.iter().any(|n| n.players.contains(u)))
                .filter(|(u, _)| {
                    accounts
                        .discord_of(**u)
                        .and_then(|d| cache.voice_states.get(d))
                        .is_some_and(|c| cache.parent_of(c) == Some(cfg.category.as_str()))
                })
                .map(|(u, _)| *u)
                .collect();
            if !within.is_empty() {
                let occupied = cache.channel_info.values().filter(|i| i.parent.as_deref() == Some(cfg.category.as_str())).count();
                if occupied >= CATEGORY_LIMIT {
                    continue;
                }
                let mut players = within;
                players.insert(uuid);
                self.next_id += 1;
                let id = self.next_id;
                self.networks.push(Network { id, players, channel: None, initialized: false });
                out.actions.push(Action::CreateNetwork {
                    network: id,
                    name: random_uuid(id),
                    overwrites: Self::network_overwrites(cfg, &guild.id, bot),
                });
            }
        }

        // Everyone in the lobby or in a network's channel goes where they belong.
        let channels: HashSet<&str> =
            self.networks.iter().filter_map(|n| n.channel.as_deref()).filter(|c| cache.channels.contains_key(*c)).collect();
        let mut members: Vec<(&String, &String)> =
            cache.voice_states.iter().filter(|(_, c)| **c == cfg.lobby || channels.contains(c.as_str())).collect();
        members.sort();
        for (user, current) in members {
            let network = accounts.uuid_of(user).and_then(|u| self.network_of(u));
            let target = match network {
                Some(n) => match n.channel.as_deref().filter(|c| cache.channels.contains_key(*c)) {
                    Some(c) => c.to_string(),
                    // Not created yet: wait for the next tick.
                    None => continue,
                },
                None => cfg.lobby.clone(),
            };
            if *current == target {
                continue;
            }
            if self.recent.get(user.as_str()).is_some_and(|(t, at)| *t == target && now.duration_since(*at) < MOVE_GRACE) {
                continue;
            }
            self.recent.insert(user.clone(), (target.clone(), now));
            out.actions.push(Action::Move { user: user.clone(), channel: target });
        }
        self.recent.retain(|_, (_, at)| now.duration_since(*at) < MOVE_GRACE * 4);

        // Networks nobody is in any more lose their channel.
        let mut delete = Vec::new();
        self.networks.retain(|n| {
            if !n.players.is_empty() {
                return true;
            }
            match n.channel.as_deref().filter(|c| cache.channels.contains_key(*c)) {
                Some(c) if !cache.voice_states.values().any(|v| v == c) => {
                    delete.push(c.to_string());
                    false
                }
                _ => true,
            }
        });
        out.actions.extend(delete.into_iter().map(|channel| Action::Delete { channel }));
        out
    }

    /// `shutdown`: everyone in a network's channel goes back to the lobby, then the channel is deleted.
    pub fn shutdown_actions(&mut self, cfg: &VoiceSettings, cache: &Cache) -> Vec<Action> {
        let mut out = Vec::new();
        for n in &self.networks {
            let Some(channel) = n.channel.as_deref() else { continue };
            for (user, current) in &cache.voice_states {
                if current == channel {
                    out.push(Action::Move { user: user.clone(), channel: cfg.lobby.clone() });
                }
            }
            out.push(Action::Delete { channel: channel.to_string() });
        }
        self.networks.clear();
        out
    }
}

/// Performs one action on Discord. A failure is logged and does not stop the module.
fn perform(shared: &Shared, action: Action) {
    let cfg = &shared.config.voice;
    let guild = {
        let st = shared.lock_state();
        Voice::guild_id(&st.cache, cfg).map(str::to_string)
    };
    let Some(guild) = guild else { return };
    let warn = |what: &str, e: String| log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Voice: {what}: {e}");
    match action {
        Action::CreateNetwork { network, name, overwrites } => match shared.rest.create_voice_channel(&guild, &name, &cfg.category, overwrites) {
            Ok(id) => {
                {
                    // The cache learns of the channel now, so the next tick does not take it for a deleted one.
                    let mut st = shared.lock_state();
                    st.cache.channels.insert(id.clone(), (name, guild.clone()));
                    st.cache.channel_info.insert(id.clone(), ChannelInfo { kind: 2, parent: Some(cfg.category.clone()), overwrites: Vec::new() });
                }
                shared.voice.lock().unwrap_or_else(|e| e.into_inner()).set_channel(network, &id);
            }
            Err(e) => {
                log::error!(target: "horizon_discordsrv", "[DiscordSRV] Failed to create a voice network: {e}");
                shared.voice.lock().unwrap_or_else(|e| e.into_inner()).drop_network(network);
            }
        },
        Action::Move { user, channel } => {
            if let Err(e) = shared.rest.move_member(&guild, &user, &channel) {
                warn("move", e);
            }
        }
        Action::Delete { channel } => {
            if let Err(e) = shared.rest.delete_channel(&channel) {
                warn("delete", e);
            }
        }
        Action::Mute { user, mute } => {
            if let Err(e) = shared.rest.set_member_mute(&guild, &user, mute) {
                warn("mute", e);
            }
        }
        Action::LobbyOverride { allow, deny } => {
            if let Err(e) = shared.rest.put_overwrite(&cfg.lobby, &guild, 0, allow, deny) {
                warn("lobby overwrite", e);
            }
        }
    }
}

fn step(shared: &Shared) {
    let cfg = &shared.config.voice;
    let tick = {
        let st = shared.lock_state();
        let mut voice = shared.voice.lock().unwrap_or_else(|e| e.into_inner());
        voice.tick(cfg, &st.cache, &st.accounts, Instant::now())
    };
    if let Some(guild) = tick.fetch_bot_roles {
        let bot = shared.lock_state().cache.bot_id.clone();
        if !bot.is_empty() {
            match shared.rest.member_roles(&guild, &bot) {
                Ok(Some(roles)) => shared.lock_state().cache.members.entry(bot).or_default().roles = roles,
                Ok(None) => {}
                Err(e) => log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Voice: the bot's roles: {e}"),
            }
        }
    }
    for line in tick.errors {
        log::error!(target: "horizon_discordsrv", "[DiscordSRV] {line}");
    }
    for action in tick.actions {
        perform(shared, action);
    }
}

/// The voice thread: a pass every `Tick speed` game ticks until the bridge stops, then the networks are closed.
pub fn run(shared: &Shared) {
    let cfg = &shared.config.voice;
    if cfg.category.trim().is_empty() || cfg.lobby.trim().is_empty() {
        log::error!(target: "horizon_discordsrv", "[DiscordSRV] Voice is on but `Voice category` or `Lobby channel` is not set: the voice module does nothing");
    }
    log::info!(target: "horizon_discordsrv", "[DiscordSRV] Enabling voice module");
    let every = Duration::from_millis(50 * cfg.tick_speed.max(1));
    while !shared.stop.load(Ordering::Relaxed) {
        step(shared);
        let until = Instant::now() + every;
        while Instant::now() < until && !shared.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(25).min(every));
        }
    }
    let actions = {
        let st = shared.lock_state();
        shared.voice.lock().unwrap_or_else(|e| e.into_inner()).shutdown_actions(cfg, &st.cache)
    };
    for action in actions {
        perform(shared, action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discordsrv_bridge::guild::tests::sample;
    use serde_json::json;

    const BOT: &str = "1";
    const CATEGORY: &str = "900";
    const LOBBY: &str = "901";

    fn cfg() -> VoiceSettings {
        VoiceSettings { enabled: true, category: CATEGORY.into(), lobby: LOBBY.into(), ..VoiceSettings::default() }
    }

    /// A guild "100" whose bot (id 1, role 210 with the voice permissions) has a category, a lobby that denies
    /// @everyone speaking and the given linked players sitting in the lobby.
    fn world() -> Cache {
        let mut c = sample();
        let manage = perm::VIEW_CHANNEL | perm::MOVE_MEMBERS | perm::MANAGE_ROLES | perm::MANAGE_CHANNELS | perm::MUTE_MEMBERS;
        c.apply(
            "GUILD_CREATE",
            &json!({
                "id": "100", "owner_id": "7", "name": "Guild",
                "roles": [
                    { "id": "100", "name": "@everyone", "color": 0, "position": 0, "permissions": (perm::VIEW_CHANNEL | perm::CONNECT | perm::SPEAK).to_string() },
                    { "id": "210", "name": "Bot", "color": 0, "position": 4, "permissions": manage.to_string() },
                ],
                "channels": [
                    { "id": CATEGORY, "name": "voice", "type": 4 },
                    { "id": LOBBY, "name": "lobby", "type": 2, "parent_id": CATEGORY,
                      "permission_overwrites": [{ "id": "100", "type": 0, "allow": "0", "deny": perm::SPEAK.to_string() }] },
                ],
                "members": [{ "user": { "id": BOT, "username": "Bot" }, "roles": ["210"] }],
            }),
        );
        c
    }

    fn join(c: &mut Cache, user: &str, channel: &str) {
        c.apply("VOICE_STATE_UPDATE", &json!({ "user_id": user, "channel_id": channel, "guild_id": "100" }));
    }

    fn accounts(dir: &str, pairs: &[(&str, u128)]) -> Accounts {
        let path = std::env::temp_dir().join(format!("discordsrv-voice-{dir}-{}", std::process::id()));
        let mut a = Accounts::load(path);
        for (d, u) in pairs {
            a.link(*u, d).unwrap();
        }
        a
    }

    fn at(x: f64, y: f64, z: f64) -> VoicePlayer {
        VoicePlayer { world: "minecraft:overworld".into(), pos: [x, y, z], alive: true }
    }

    #[test]
    fn uuid_names_are_recognised() {
        assert!(is_uuid_name("123e4567-e89b-42d3-a456-426614174000"));
        assert!(!is_uuid_name("lobby"));
        assert!(!is_uuid_name("123e4567-e89b-42d3-a456-42661417400g"));
        assert!(is_uuid_name(&random_uuid(7)));
        assert_ne!(random_uuid(1), random_uuid(1));
    }

    #[test]
    fn the_default_distances_are_the_originals() {
        let c = VoiceSettings::default();
        assert_eq!((c.network.vertical_strength, c.network.horizontal_strength, c.network.falloff), (40.0, 80.0, 5.0));
        assert_eq!(c.tick_speed, 5);
        assert!(c.mute_bypassers && !c.network.channels_visible);
    }

    #[test]
    fn two_players_in_range_get_a_network_and_far_ones_do_not() {
        let mut cache = world();
        // Both sit in the lobby, which is a channel of the category.
        join(&mut cache, "11", LOBBY);
        join(&mut cache, "12", LOBBY);
        let acc = accounts("range", &[("11", 1), ("12", 2)]);
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(79.0, 64.0, 0.0))]);
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let created: Vec<_> = t.actions.iter().filter(|a| matches!(a, Action::CreateNetwork { .. })).collect();
        assert_eq!(created.len(), 1, "{:?}", t.actions);
        assert_eq!(v.networks.len(), 1);
        assert_eq!(v.networks[0].players.len(), 2);
        // Out of range: nothing is made.
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(81.0, 64.0, 0.0))]);
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(!t.actions.iter().any(|a| matches!(a, Action::CreateNetwork { .. })));
        // Too far apart vertically.
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 0.0, 0.0)), (2, at(1.0, 41.0, 0.0))]);
        assert!(!v.tick(&cfg(), &cache, &acc, Instant::now()).actions.iter().any(|a| matches!(a, Action::CreateNetwork { .. })));
    }

    #[test]
    fn the_network_channel_is_made_with_the_originals_overwrites() {
        let mut cache = world();
        join(&mut cache, "11", LOBBY);
        join(&mut cache, "12", LOBBY);
        let acc = accounts("overwrites", &[("11", 1), ("12", 2)]);
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0))]);
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        let Some(Action::CreateNetwork { name, overwrites, .. }) = t.actions.iter().find(|a| matches!(a, Action::CreateNetwork { .. })) else {
            panic!("no network: {:?}", t.actions)
        };
        assert!(is_uuid_name(name));
        // @everyone: speak allowed, connect and view denied; the bot: view, connect, move.
        assert_eq!(overwrites[0]["allow"], perm::SPEAK.to_string());
        assert_eq!(overwrites[0]["deny"], (perm::CONNECT | perm::VIEW_CHANNEL).to_string());
        assert_eq!(overwrites[1]["allow"], (perm::VIEW_CHANNEL | perm::CONNECT | perm::MOVE_MEMBERS).to_string());
        let mut visible = cfg();
        visible.network.channels_visible = true;
        let o = Voice::network_overwrites(&visible, "100", "1");
        assert_eq!(o[0]["allow"], (perm::SPEAK | perm::VIEW_CHANNEL).to_string());
        assert_eq!(o[0]["deny"], perm::CONNECT.to_string());
    }

    #[test]
    fn players_in_the_lobby_move_into_their_networks_channel_and_back() {
        let mut cache = world();
        join(&mut cache, "11", LOBBY);
        join(&mut cache, "12", LOBBY);
        cache.apply("CHANNEL_CREATE", &json!({ "id": "950", "name": "net", "guild_id": "100", "type": 2, "parent_id": CATEGORY }));
        let acc = accounts("moves", &[("11", 1), ("12", 2)]);
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0))]);
        let _ = v.tick(&cfg(), &cache, &acc, Instant::now());
        let id = v.networks[0].id;
        v.set_channel(id, "950");
        let t = v.tick(&cfg(), &cache, &acc, Instant::now() + Duration::from_secs(10));
        let moves: Vec<_> = t.actions.iter().filter(|a| matches!(a, Action::Move { channel, .. } if channel == "950")).collect();
        assert_eq!(moves.len(), 2, "{:?}", t.actions);
        // One leaves the game: the other has nobody left, the network is emptied by the leave.
        join(&mut cache, "11", "950");
        join(&mut cache, "12", "950");
        v.update_players(vec![(2, at(1.0, 64.0, 0.0))]);
        assert_eq!(v.network_of(2).map(|n| n.players.len()), Some(1));
    }

    #[test]
    fn a_player_who_moves_beyond_the_falloff_leaves_the_network() {
        let mut cache = world();
        join(&mut cache, "11", LOBBY);
        join(&mut cache, "12", LOBBY);
        let acc = accounts("falloff", &[("11", 1), ("12", 2), ("13", 3)]);
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0)), (3, at(2.0, 64.0, 0.0))]);
        join(&mut cache, "13", LOBBY);
        let _ = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert_eq!(v.networks.len(), 1);
        let members = v.networks[0].players.len();
        assert!(members >= 2);
        // 85 blocks away is inside strength + falloff (85): still connected; 86 is not.
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0)), (3, at(85.0, 64.0, 0.0))]);
        let _ = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(v.network_of(3).is_some(), "falloff keeps the player");
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0)), (3, at(87.0, 64.0, 0.0))]);
        let _ = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(v.network_of(3).is_none(), "beyond the falloff the player leaves");
    }

    #[test]
    fn an_empty_network_is_deleted_once_nobody_sits_in_its_channel() {
        let mut cache = world();
        cache.apply("CHANNEL_CREATE", &json!({ "id": "950", "name": "net", "guild_id": "100", "type": 2, "parent_id": CATEGORY }));
        let acc = accounts("delete", &[]);
        let mut v = Voice::default();
        v.networks.push(Network { id: 1, players: HashSet::new(), channel: Some("950".into()), initialized: true });
        join(&mut cache, "21", "950");
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        // Someone is still in it: kept, and the unlinked person is sent to the lobby.
        assert!(!t.actions.iter().any(|a| matches!(a, Action::Delete { .. })));
        assert!(t.actions.contains(&Action::Move { user: "21".into(), channel: LOBBY.into() }));
        join(&mut cache, "21", LOBBY);
        let t = v.tick(&cfg(), &cache, &acc, Instant::now() + Duration::from_secs(10));
        assert!(t.actions.contains(&Action::Delete { channel: "950".into() }), "{:?}", t.actions);
        assert!(v.networks.is_empty());
    }

    #[test]
    fn channels_left_by_an_earlier_run_are_found_and_emptied() {
        let mut cache = world();
        cache.apply(
            "CHANNEL_CREATE",
            &json!({ "id": "951", "name": "123e4567-e89b-42d3-a456-426614174000", "guild_id": "100", "type": 2, "parent_id": CATEGORY }),
        );
        cache.apply("CHANNEL_CREATE", &json!({ "id": "952", "name": "hangout", "guild_id": "100", "type": 2, "parent_id": CATEGORY }));
        let acc = accounts("scan", &[]);
        let mut v = Voice::default();
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(t.actions.contains(&Action::Delete { channel: "951".into() }), "{:?}", t.actions);
        assert!(!t.actions.contains(&Action::Delete { channel: "952".into() }));
    }

    #[test]
    fn missing_permissions_stop_the_module_with_an_error_every_thirty_seconds() {
        let mut cache = world();
        cache.apply("GUILD_ROLE_UPDATE", &json!({ "guild_id": "100", "role": { "id": "210", "name": "Bot", "color": 0, "position": 4, "permissions": "1024" } }));
        let acc = accounts("perms", &[]);
        let mut v = Voice::default();
        let now = Instant::now();
        let t = v.tick(&cfg(), &cache, &acc, now);
        assert!(t.actions.is_empty());
        assert!(t.errors.iter().any(|e| e.contains("Move Members") && e.contains("voice lobby")), "{:?}", t.errors);
        assert!(t.errors.iter().any(|e| e.contains("Manage Channels") && e.contains("voice category")));
        assert!(v.tick(&cfg(), &cache, &acc, now + Duration::from_secs(5)).errors.is_empty());
        assert!(!v.tick(&cfg(), &cache, &acc, now + Duration::from_secs(31)).errors.is_empty());
    }

    #[test]
    fn the_lobby_must_deny_speaking_to_everyone() {
        let mut cache = world();
        cache.apply("CHANNEL_UPDATE", &json!({ "id": LOBBY, "name": "lobby", "guild_id": "100", "type": 2, "parent_id": CATEGORY,
            "permission_overwrites": [{ "id": "100", "type": 0, "allow": perm::SPEAK.to_string(), "deny": "0" }] }));
        let acc = accounts("override", &[]);
        let mut v = Voice::default();
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(t.actions.contains(&Action::LobbyOverride { allow: 0, deny: perm::SPEAK }), "{:?}", t.actions);
        // Already denied (the world() lobby): no action.
        let mut v = Voice::default();
        assert!(!world_tick(&mut v).actions.iter().any(|a| matches!(a, Action::LobbyOverride { .. })));
    }

    fn world_tick(v: &mut Voice) -> Tick {
        let acc = accounts("noop", &[]);
        v.tick(&cfg(), &world(), &acc, Instant::now())
    }

    #[test]
    fn someone_who_can_speak_in_the_lobby_is_muted_and_unmuted_when_they_leave_it() {
        let mut cache = world();
        // The user holds an administrator role: the lobby's deny does not bind them.
        cache.apply("GUILD_ROLE_CREATE", &json!({ "guild_id": "100", "role": { "id": "220", "name": "Admin", "color": 0, "position": 3,
            "permissions": perm::ADMINISTRATOR.to_string() } }));
        cache.apply("VOICE_STATE_UPDATE", &json!({ "user_id": "31", "channel_id": LOBBY, "guild_id": "100",
            "member": { "user": { "id": "31", "username": "mod" }, "roles": ["220"] } }));
        let acc = accounts("mute", &[]);
        let mut v = Voice::default();
        let (user, old, new) = cache.voice_change.take().unwrap();
        v.on_voice_change(&cfg(), &cache, &acc, &user, old.as_deref(), new.as_deref());
        let t = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(t.actions.contains(&Action::Mute { user: "31".into(), mute: true }), "{:?}", t.actions);
        // They go to another channel: unmuted.
        cache.apply("VOICE_STATE_UPDATE", &json!({ "user_id": "31", "channel_id": "952", "guild_id": "100" }));
        let (user, old, new) = cache.voice_change.take().unwrap();
        v.on_voice_change(&cfg(), &cache, &acc, &user, old.as_deref(), new.as_deref());
        let t = v.tick(&cfg(), &cache, &acc, Instant::now() + Duration::from_secs(10));
        assert!(t.actions.contains(&Action::Mute { user: "31".into(), mute: false }), "{:?}", t.actions);
        // A plain member (only @everyone) is not muted.
        let mut off = cfg();
        off.mute_bypassers = false;
        cache.apply("VOICE_STATE_UPDATE", &json!({ "user_id": "31", "channel_id": LOBBY, "guild_id": "100" }));
        let (user, old, new) = cache.voice_change.take().unwrap();
        let mut v = Voice::default();
        v.on_voice_change(&off, &cache, &acc, &user, old.as_deref(), new.as_deref());
        assert!(v.pending.is_empty());
    }

    #[test]
    fn leaving_the_category_takes_the_player_out_of_their_networks() {
        let mut cache = world();
        join(&mut cache, "11", LOBBY);
        join(&mut cache, "12", LOBBY);
        let acc = accounts("leave", &[("11", 1), ("12", 2)]);
        let mut v = Voice::default();
        v.update_players(vec![(1, at(0.0, 64.0, 0.0)), (2, at(1.0, 64.0, 0.0))]);
        let _ = v.tick(&cfg(), &cache, &acc, Instant::now());
        assert!(v.network_of(1).is_some());
        cache.apply("CHANNEL_CREATE", &json!({ "id": "960", "name": "elsewhere", "guild_id": "100", "type": 2, "parent_id": "999" }));
        cache.apply("VOICE_STATE_UPDATE", &json!({ "user_id": "11", "channel_id": "960", "guild_id": "100" }));
        let (user, old, new) = cache.voice_change.take().unwrap();
        v.on_voice_change(&cfg(), &cache, &acc, &user, old.as_deref(), new.as_deref());
        assert!(v.network_of(1).is_none());
        assert!(v.network_of(2).is_some());
    }

    #[test]
    fn shutdown_sends_everyone_to_the_lobby_and_deletes_the_channels() {
        let mut cache = world();
        join(&mut cache, "11", "950");
        let mut v = Voice::default();
        v.networks.push(Network { id: 1, players: HashSet::from([1]), channel: Some("950".into()), initialized: true });
        let actions = v.shutdown_actions(&cfg(), &cache);
        assert_eq!(
            actions,
            vec![Action::Move { user: "11".into(), channel: LOBBY.into() }, Action::Delete { channel: "950".into() }]
        );
        assert!(v.networks.is_empty());
    }

    #[test]
    fn the_bots_roles_are_fetched_when_unknown() {
        let mut cache = world();
        cache.members.remove(BOT);
        let acc = accounts("fetch", &[]);
        let mut v = Voice::default();
        assert_eq!(v.tick(&cfg(), &cache, &acc, Instant::now()).fetch_bot_roles.as_deref(), Some("100"));
    }
}
