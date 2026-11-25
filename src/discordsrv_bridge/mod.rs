//! DiscordSRV for a Horizon dedicated server: the console channel (the server's log streamed
//! to a Discord channel, commands typed there run by the console), the chat channels (game chat
//! both ways, join/leave/death/advancement notifications, start/stop messages, the player list,
//! canned responses, topics and channel names, the watchdog), account linking, and the bot's
//! presence. Everything that touches the network runs on threads of its own; the server thread
//! hands over its TPS, its players and the game's events, and takes queued commands and lines.
pub mod alerts;
pub mod chat;
pub mod commands;
pub mod config;
pub mod console;
pub mod debugger;
pub mod embed;
pub mod feed;
pub mod format;
pub mod gateway;
pub mod guild;
pub mod linking;
pub mod original;
pub mod placeholders;
pub mod relay;
pub mod requirelink;
pub mod rest;
pub mod server;
pub mod sync;
pub mod voice;
pub mod ws;
pub mod yaml;

use std::path::PathBuf;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;

use config::Config;
pub use server::Player;

/// Environment variable that points Discord's REST base at a local fake for tests. Not for a live server:
/// the bot token is sent to whatever it names.
pub const API_BASE_ENV: &str = "HORIZON_DISCORDSRV_API_BASE";

/// Console lines kept while Discord is slow; older ones are dropped past this.
const MAX_PENDING_LINES: usize = 5000;
/// Players are read from the server every this many ticks.
const PLAYER_SNAPSHOT_TICKS: u64 = 20;

/// State shared by the server thread and the bridge's threads.
pub struct Shared {
    pub config: Config,
    pub token: String,
    pub rest: rest::Rest,
    pub stop: AtomicBool,
    pub ready: AtomicBool,
    /// Ticks per second, as f32 bits, set by the server thread.
    tps: AtomicU32,
    commands: Mutex<VecDeque<String>>,
    /// Console channel commands, whose answer goes back to the channel.
    console_queue: Mutex<VecDeque<String>>,
    /// Lines for the console channel that are no log lines (answers, refusals).
    replies: Mutex<Vec<String>>,
    /// Commands typed in a chat channel (`!c`) that wait for their answer: (command, channel).
    chat_console: Mutex<VecDeque<(String, String)>>,
    usage_log_dir: PathBuf,
    /// The first tick happened (the server has started).
    started: AtomicBool,
    /// Milliseconds after `epoch` of the last tick, for the watchdog.
    last_tick_ms: AtomicU64,
    epoch: Instant,
    pub server: server::ServerInfo,
    pub version: String,
    chat_filters: chat::ChatFilters,
    /// Which `DiscordGameStatus` entry the presence shows (see `gateway::status_entry`).
    pub status_index: AtomicUsize,
    /// Set by a reload on the old bridge: its threads end without the shutdown message and topics.
    quiet_stop: AtomicBool,
    /// Set on the bridge a reload starts: the startup message was sent by the old one.
    suppress_startup: AtomicBool,
    state: Mutex<State>,
    jobs: Mutex<VecDeque<relay::Job>>,
    to_game: Mutex<TickWork>,
    /// The parsed `Alerts` (alerts.yml).
    alerts: Vec<alerts::Alert>,
    /// The voice module's networks (voice.yml); idle when `Voice enabled` is off.
    voice: Mutex<voice::Voice>,
    /// A placeholder asked for the member count: the chat thread fetches it from Discord about once a minute.
    counts_wanted: AtomicBool,
}

/// What the gateway and the game told, guarded together.
pub struct State {
    pub cache: guild::Cache,
    pub accounts: linking::Accounts,
    /// The players of the last snapshot.
    pub players: Vec<Player>,
    /// Players whose login found no player file: their join is a first join.
    new_players: HashSet<u128>,
    ticks: u64,
}

/// What the server thread does this tick for the bridge.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickWork {
    /// Console commands of the linking (run as they are).
    pub commands: Vec<String>,
    /// Console channel commands: run with their answer kept, handed to [`console_output`].
    pub console: Vec<String>,
    /// `§`-coded lines for every player's chat (Discord messages).
    pub broadcasts: Vec<String>,
    /// `§`-coded lines for one player.
    pub tells: Vec<(u128, String)>,
    /// Names of the players a Discord message mentioned as `@name` (`MinecraftMentionSound`): they
    /// hear the note-block pling.
    pub mentions: Vec<String>,
    /// Players to kick, with the `§`-coded text: they unlinked while linking is required.
    pub kicks: Vec<(u128, String)>,
    /// A `/discord resync`, a new link or the cycle timer wants the groups of these players
    /// (every linked player when `resync_all`).
    pub resync: Vec<u128>,
    pub resync_all: bool,
    /// A Discord role edit wants the groups of these players to follow (Discord decides).
    pub resync_to_minecraft: Vec<u128>,
    /// Groups to give (`true`) or take from a player: Discord -> Minecraft synchronisation. The server
    /// thread writes them through the permissions provider.
    pub group_writes: Vec<(u128, String, bool)>,
}

/// The players of `names` that `message` mentions as `@name`, as `PlayerUtil.notifyPlayersOfMentions`
/// finds them: the colour-stripped, lower-cased text split on anything but letters, digits, `_` and `@`.
pub fn mentioned(message: &str, names: &[String]) -> Vec<String> {
    let plain = console::strip_colors(message).to_lowercase();
    let cleaned: String = plain.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '@' { c } else { ' ' }).collect();
    let words: HashSet<&str> = cleaned.split_whitespace().collect();
    names.iter().filter(|n| words.contains(format!("@{}", n.to_lowercase()).as_str())).cloned().collect()
}

struct Running {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

static RUNNING: OnceLock<Mutex<Option<Running>>> = OnceLock::new();

/// What the log hook hands over for the console channel.
static LOG_FEED: feed::Feed = feed::Feed::new(MAX_PENDING_LINES);

/// What a console preprocessor makes of a console channel message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preprocess {
    /// Run this command (the message as it came, or another one).
    Run(String),
    /// Run nothing; `reply` is posted to the console channel.
    Refuse { reply: String },
}

/// `f(author_id, command)`, see [`set_console_preprocessor`].
pub type Preprocessor = Box<dyn Fn(&str, &str) -> Preprocess + Send + Sync>;

/// The registered preprocessors with their handle ids, in registration order.
static PREPROCESSORS: Mutex<Vec<(u64, std::sync::Arc<Preprocessor>)>> = Mutex::new(Vec::new());
static NEXT_PREPROCESSOR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Handle of one preprocessor, for [`remove_console_preprocessor`] (DiscordSRV's `api.unsubscribe`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreprocessorHandle(u64);

/// Lets another mod look at each console channel message before it runs (DiscordSRV's console
/// command pre-process event): `f(author_id, command)` gets the author's Discord user ID and the
/// command without its leading slash, after the bot check and before the command list, so a
/// command it returns must still pass the list. This call replaces every earlier preprocessor
/// with `f`; use [`add_console_preprocessor`] to keep the others. It runs on the bridge's gateway
/// thread and must not call back into this function.
pub fn set_console_preprocessor(f: Preprocessor) {
    let mut all = PREPROCESSORS.lock().unwrap_or_else(|e| e.into_inner());
    all.clear();
    all.push((NEXT_PREPROCESSOR.fetch_add(1, Ordering::Relaxed), std::sync::Arc::new(f)));
}

/// Adds a preprocessor after the others (DiscordSRV `api.subscribe`): they run in order, each one
/// gets the command the one before returned, and the first `Refuse` ends the chain.
pub fn add_console_preprocessor(f: Preprocessor) -> PreprocessorHandle {
    let id = NEXT_PREPROCESSOR.fetch_add(1, Ordering::Relaxed);
    PREPROCESSORS.lock().unwrap_or_else(|e| e.into_inner()).push((id, std::sync::Arc::new(f)));
    PreprocessorHandle(id)
}

/// Removes one preprocessor (DiscordSRV `api.unsubscribe`, at the owner's stop). False when it was gone.
pub fn remove_console_preprocessor(handle: PreprocessorHandle) -> bool {
    let mut all = PREPROCESSORS.lock().unwrap_or_else(|e| e.into_inner());
    let before = all.len();
    all.retain(|(id, _)| *id != handle.0);
    all.len() != before
}

/// A console channel command ran (`native_run_command_captured`): its answer goes back to the
/// console channel with the next batch. `output` holds the answer's lines as plain text.
pub fn console_output(command: &str, ran: bool, output: Vec<String>) {
    if let Some(sh) = shared() {
        // A command typed in a chat channel (`!c`) answers there, saying nothing when it printed nothing.
        let pending = {
            let mut waiting = sh.chat_console.lock().unwrap_or_else(|e| e.into_inner());
            let at = waiting.iter().position(|(c, _)| c == command);
            at.and_then(|i| waiting.remove(i))
        };
        match pending {
            Some((_, channel)) => sh.chat_console_answer(&channel, &output),
            None => sh.reply(output_lines(command, ran, output)),
        }
    }
}

/// The lines a command's answer gives in the channel: its output, colour codes removed; for a
/// command that answered nothing (vanilla's commands, until the engine keeps their output) one
/// line saying whether it ran.
pub fn output_lines(command: &str, ran: bool, output: Vec<String>) -> Vec<String> {
    let lines: Vec<String> = output
        .iter()
        .flat_map(|l| l.split('\n'))
        .map(console::strip_colors)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if !lines.is_empty() {
        return lines;
    }
    let verdict = if ran { "ran, no output" } else { "did not run" };
    vec![format!("> {command}: {verdict}")]
}

fn running() -> &'static Mutex<Option<Running>> {
    RUNNING.get_or_init(|| Mutex::new(None))
}

fn shared() -> Option<Arc<Shared>> {
    running().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| Arc::clone(&r.shared))
}

/// A message the server logged (the log hook, on the log's writer thread): kept for the console
/// channel. `level` is `INFO`, `WARN` or `ERROR`, `message` the text as logged.
pub fn log_line(level: &str, message: &str) {
    LOG_FEED.push(level, message);
}

/// Nothing will forward the log (not a dedicated server): the log hook keeps nothing.
pub fn log_feed_off() {
    LOG_FEED.close();
}

/// Starts the bridge from `<config_dir>/discordsrv/config.toml` (written with the defaults when
/// missing), forwarding what [`log_line`] hears. Nothing starts without a console or chat channel
/// and a token.
pub fn start(config_dir: &Path, server_version: String) {
    let mut slot = running().lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return;
    }
    *LAST_START.lock().unwrap_or_else(|e| e.into_inner()) = Some((config_dir.to_path_buf(), server_version.clone()));
    if !start_inner(&mut slot, config_dir, server_version, false) {
        LOG_FEED.close();
    }
}

/// Where [`start`] was told to read its config, for [`reload`].
static LAST_START: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// `/discord reload`: reads the config again and restarts the bridge with it, as DiscordSRV's
/// `CommandReload` does (config, channels, regexes, role aliases, channel updater). The old threads
/// end without the shutdown message and topics, the new ones without the startup message; the
/// gateway reconnects. Also starts a bridge that was off because of the old config. The caller is
/// the server thread (a command), so nothing ticks meanwhile; the wait is at most five seconds.
/// `Ok(true)`: the bridge runs again; `Ok(false)`: the new config switched it off (the log says why).
pub fn reload() -> Result<bool, String> {
    let (dir, version) =
        LAST_START.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("DiscordSRV was not started on this server")?;
    let old = running().lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = old {
        r.shared.quiet_stop.store(true, Ordering::Relaxed);
        r.shared.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && r.threads.iter().any(|t| !t.is_finished()) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    LOG_FEED.reopen();
    let mut slot = running().lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() && !start_inner(&mut slot, &dir, version, true) {
        LOG_FEED.close();
    }
    Ok(slot.is_some())
}

/// [`start`]; false when the console channel will not run, so the log feed can close.
/// `reload`: a restart of a bridge that already ran (no startup message).
fn start_inner(slot: &mut Option<Running>, config_dir: &Path, server_version: String, reload: bool) -> bool {
    let dir = config_dir.join("discordsrv");
    let path = dir.join("config.toml");
    // The original DiscordSRV files (`plugins/DiscordSRV/config.yml` in the server folder, the
    // engine's working directory, read as they are) win; the TOML file is the fallback.
    let original_dir = std::env::current_dir().unwrap_or_default().join("plugins").join("DiscordSRV");
    let (mut config, yaml_token, source) = match original::load(&original_dir) {
        Ok(Some(o)) => {
            for feature in &o.unported {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Switched on in the original, not ported: {feature}");
            }
            (o.config, o.token, original_dir.join("config.yml"))
        }
        Ok(None) => {
            if !path.exists() {
                let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(&path, config::DEFAULT_CONFIG));
            }
            match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Config::parse(&t)) {
                Ok(c) => (c, String::new(), path.clone()),
                Err(e) => {
                    log::error!(target: "horizon_discordsrv", "[DiscordSRV] {}: {e}", path.display());
                    return false;
                }
            }
        }
        Err(e) => {
            log::error!(target: "horizon_discordsrv", "[DiscordSRV] Original config not read ({e}): not connecting");
            return false;
        }
    };
    if let Some(base) = std::env::var(API_BASE_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        log::warn!(target: "horizon_discordsrv", "[DiscordSRV] {API_BASE_ENV} is set: REST calls go to the test endpoint");
        config.api_base_url = base;
    }
    if config.discord_console_channel_padding > 0 {
        log::warn!(target: "horizon_discordsrv", "[DiscordSRV] DiscordConsoleChannelPadding is not applied (see GAPS.md)");
    }
    let forced = config.chat.forced_language.trim().to_ascii_lowercase();
    if !matches!(forced.as_str(), "" | "none" | "en" | "english") {
        log::warn!(target: "horizon_discordsrv", "[DiscordSRV] ForcedLanguage is not applied: only English exists in this port");
    }
    let zone = config.chat.timezone.trim().to_ascii_lowercase();
    if !matches!(zone.as_str(), "" | "default") {
        log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Timezone is not applied: the server's time zone is used");
    }
    let console_on = !config.discord_console_channel_id.trim().is_empty();
    if !console_on && !config.chat.has_channels() {
        log::info!(target: "horizon_discordsrv", "[DiscordSRV] No console or chat channel set in {}: not connecting", source.display());
        return false;
    }
    // The token never leaves this function: it is not logged, not even its length.
    let token = if !yaml_token.is_empty() {
        yaml_token
    } else {
        match fs::read_to_string(dir.join(&config.bot_token_file)) {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => {
                log::error!(target: "horizon_discordsrv", "[DiscordSRV] No bot token in {}: not connecting", source.display());
                return false;
            }
        }
    };
    let compiled = console::Formatter::new(&config).and_then(|f| chat::ChatFilters::new(&config.chat).map(|c| (f, c)));
    let (formatter, chat_filters) = match compiled {
        Ok(x) => x,
        Err(e) => {
            log::error!(target: "horizon_discordsrv", "[DiscordSRV] {e}");
            return false;
        }
    };
    let home = config_dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let accounts_file = if config.chat.accounts_file.trim().is_empty() { "accounts.aof" } else { &config.chat.accounts_file };
    let accounts = linking::Accounts::load(dir.join(accounts_file));
    let alert_list = build_alerts(&config);
    let shared = Arc::new(Shared {
        rest: rest::Rest::with_proxy(&config.api_base_url, &token, config.proxy().as_ref()),
        config,
        token,
        stop: AtomicBool::new(false),
        ready: AtomicBool::new(false),
        tps: AtomicU32::new(20f32.to_bits()),
        commands: Mutex::new(VecDeque::new()),
        console_queue: Mutex::new(VecDeque::new()),
        replies: Mutex::new(Vec::new()),
        chat_console: Mutex::new(VecDeque::new()),
        usage_log_dir: dir,
        started: AtomicBool::new(reload),
        last_tick_ms: AtomicU64::new(0),
        epoch: Instant::now(),
        server: server::ServerInfo::load(&home),
        version: server_version,
        chat_filters,
        status_index: AtomicUsize::new(0),
        quiet_stop: AtomicBool::new(false),
        suppress_startup: AtomicBool::new(reload),
        state: Mutex::new(State {
            cache: guild::Cache::default(),
            accounts,
            players: Vec::new(),
            new_players: HashSet::new(),
            ticks: 0,
        }),
        jobs: Mutex::new(VecDeque::new()),
        to_game: Mutex::new(TickWork::default()),
        alerts: alert_list,
        voice: Mutex::new(voice::Voice::default()),
        counts_wanted: AtomicBool::new(false),
    });
    let mut threads = Vec::new();
    {
        let shared = Arc::clone(&shared);
        threads.push(std::thread::Builder::new().name("DiscordSRV - Gateway".into()).spawn(move || {
            let s = Arc::clone(&shared);
            gateway::run(shared, move |event, d| s.dispatch(event, d));
        }));
    }
    if console_on {
        let s = Arc::clone(&shared);
        threads.push(std::thread::Builder::new().name("DiscordSRV - Console".into()).spawn(move || {
            let version = s.version.clone();
            console_loop(&s, formatter, &version);
        }));
        log::info!(
            target: "horizon_discordsrv",
            "[DiscordSRV] Console forwarding started ({} lines every {}s)",
            shared.config.discord_console_channel_levels.join("/"),
            shared.config.discord_console_channel_log_refresh_rate_in_seconds
        );
    }
    if shared.config.chat.has_channels() {
        let s = Arc::clone(&shared);
        threads.push(std::thread::Builder::new().name("DiscordSRV - Chat".into()).spawn(move || relay::chat_loop(&s)));
        log::info!(target: "horizon_discordsrv", "[DiscordSRV] Chat bridge started ({} channels)", shared.config.chat.channels.len());
    }
    if shared.config.voice.enabled {
        let s = Arc::clone(&shared);
        threads.push(std::thread::Builder::new().name("DiscordSRV - Voice".into()).spawn(move || voice::run(&s)));
    }
    let threads: Vec<JoinHandle<()>> = threads.into_iter().filter_map(Result::ok).collect();
    *slot = Some(Running { shared, threads });
    console_on
}

/// Called by the server thread every tick: hands over the TPS (and every
/// [`PLAYER_SNAPSHOT_TICKS`] the players, which `players` reads) and returns what to do now.
pub fn tick(tps: f32, players: impl FnOnce() -> Vec<Player>) -> TickWork {
    let Some(sh) = shared() else {
        return TickWork::default();
    };
    sh.tps.store(tps.to_bits(), Ordering::Relaxed);
    sh.last_tick_ms.store(sh.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    sh.started.store(true, Ordering::Relaxed);
    {
        let mut st = sh.lock_state();
        if st.ticks % PLAYER_SNAPSHOT_TICKS == 0 {
            st.players = players();
        }
        st.ticks += 1;
    }
    let mut work = std::mem::take(&mut *sh.to_game.lock().unwrap_or_else(|e| e.into_inner()));
    if sh.config.chat.minecraft_mention_sound && !work.broadcasts.is_empty() {
        let names: Vec<String> = sh.lock_state().players.iter().map(|p| p.name.clone()).collect();
        for line in &work.broadcasts {
            for name in mentioned(line, &names) {
                if !work.mentions.contains(&name) {
                    work.mentions.push(name);
                }
            }
        }
    }
    if (sh.sync_enabled() || sh.config.chat.nickname_synchronization_enabled) && sync::cycle_due(sh.config.chat.group_role_synchronization_cycle_time.max(1)) {
        // GroupRoleSynchronizationCycleTime: the players on the server (not everyone linked).
        let online: Vec<u128> = sh.lock_state().players.iter().map(|p| p.uuid).collect();
        work.resync.extend(online);
    }
    work.commands.extend(sh.commands.lock().map(|mut q| q.drain(..).collect::<Vec<_>>()).unwrap_or_default());
    work.console.extend(sh.console_queue.lock().map(|mut q| q.drain(..).collect::<Vec<_>>()).unwrap_or_default());
    work
}

/// The server stops: the last console lines, the shutdown message and topics go out, then the
/// threads end (five seconds at most, so a dead network can't hold the stop up).
pub fn stop() {
    let Some(r) = running().lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    // The console loop sends what it has before it ends; later lines have no reader.
    let _closer = CloseFeedOnDrop;
    r.shared.stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && r.threads.iter().any(|t| !t.is_finished()) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A player's login was accepted, before their saved state is loaded: whether the world has a
/// file of them decides if their join is a first join.
pub fn player_login(uuid: u128) {
    if let Some(sh) = shared()
        && sh.server.is_new_player(uuid)
    {
        sh.lock_state().new_players.insert(uuid);
    }
}

/// The alerts of the config, with a line for each one that cannot work.
fn build_alerts(config: &Config) -> Vec<alerts::Alert> {
    let mut warnings = Vec::new();
    let list = alerts::parse(&config.chat.alerts, &mut warnings);
    for w in warnings {
        log::warn!(target: "horizon_discordsrv", "[DiscordSRV] {w}");
    }
    if !list.is_empty() {
        log::info!(target: "horizon_discordsrv", "[DiscordSRV] {} alert{} registered", list.len(), if list.len() == 1 { "" } else { "s" });
    }
    list
}

/// The Bukkit world name of a dimension (`world`, `world_nether`, `world_the_end`), for the alerts' `{world}`.
pub fn world_name(dimension: &str) -> String {
    shared().map(|sh| sh.server.world_name(dimension)).unwrap_or_else(|| dimension.to_string())
}

/// Whether any alert is configured (the glue builds an event only then).
pub fn alerts_on() -> bool {
    shared().is_some_and(|sh| !sh.alerts.is_empty())
}

/// A game event for the alerts: each alert it triggers has its conditions and `${...}` evaluated here, on
/// the server thread (they may ask the game), and its message queued for the chat thread.
pub fn alert_event(ev: &alerts::Event, player: Option<Player>, ping: i64) {
    let Some(sh) = shared() else { return };
    for alert in sh.alerts.iter().filter(|a| alerts::triggered(a, ev)) {
        let mut log = Vec::new();
        let template = alerts::prepare(alert, ev, &mut log);
        for line in log {
            log::error!(target: "horizon_discordsrv", "[DiscordSRV] {line}");
        }
        if let Some(template) = template {
            sh.push_job(relay::Job::Alert { channels: alert.channels.clone(), template, player: player.clone(), ping });
        }
    }
}

/// Whether the login check is on (the glue then reads the whitelist and the ban list for it).
pub fn login_check_on() -> bool {
    shared().is_some_and(|sh| sh.config.chat.require_linked_account_to_play.enabled)
}

/// `Require linked account to play`: the kick text of a login that must be refused, None to let the
/// player in. `whitelisted` and `banned` are the server's answers for the player (vanilla whitelist,
/// name ban list). The rules and their order are [`requirelink::decide`]'s; the Discord questions
/// (membership, roles) are asked of the REST API here, as DiscordSRV's `.complete()` calls are.
pub fn login_refusal(uuid: u128, name: &str, whitelisted: bool, banned: bool) -> Option<String> {
    let sh = shared()?;
    let rule = &sh.config.chat.require_linked_account_to_play;
    if !rule.enabled {
        return None;
    }
    let bot_name = sh.lock_state().cache.bot_name.clone();
    let facts = requirelink::Facts {
        name,
        whitelisted,
        banned,
        ready: sh.ready.load(Ordering::Relaxed),
        bot_name: &bot_name,
        invite: &sh.config.chat.discord_invite_link,
    };
    let mut live = LiveDiscord { sh: &sh, uuid, name };
    let mut log = Vec::new();
    let text = requirelink::decide(rule, &facts, &mut live, &mut log);
    for line in log {
        if line.starts_with("Failed to check player") || line.starts_with("Tried to authenticate") {
            log::warn!(target: "horizon_discordsrv", "[DiscordSRV] {line}");
        } else {
            debugger::log("REQUIRE_LINK", &sh.config.chat.debug, &line);
        }
    }
    text
}

/// `noticePlayerUnlink`'s exceptions for a player who unlinked: false when the whitelist bypass holds
/// or only banned players need to link and this one is not banned.
pub fn unlink_kick_applies(whitelisted: bool, banned: bool) -> bool {
    shared().is_some_and(|sh| requirelink::unlink_kick_applies(&sh.config.chat.require_linked_account_to_play, whitelisted, banned))
}

/// The Discord side of a login check, over the live link file, the guild cache and the REST API.
struct LiveDiscord<'a> {
    sh: &'a Shared,
    uuid: u128,
    name: &'a str,
}

impl requirelink::Discord for LiveDiscord<'_> {
    fn linked(&mut self) -> Option<String> {
        self.sh.lock_state().accounts.discord_of(self.uuid).map(|d| d.to_string())
    }
    fn new_code(&mut self) -> String {
        self.sh.lock_state().accounts.new_code(self.uuid, self.name)
    }
    fn bot_guilds(&self) -> Vec<String> {
        self.sh.lock_state().cache.guilds.keys().cloned().collect()
    }
    fn role_guild(&self, role: &str) -> Option<String> {
        let st = self.sh.lock_state();
        st.cache.guilds.values().find(|g| g.roles.contains_key(role)).map(|g| g.id.clone())
    }
    fn member_roles(&mut self, guild: &str, user: &str) -> Result<Option<Vec<String>>, String> {
        self.sh.rest.member_roles(guild, user)
    }
}

/// DiscordSRV's default kick texts (linking.yml, `&` codes).
const NOT_LINKED: &str = "&7You must link your &9Discord &7account to play.\n\n&7Send a DM to &b{BOT}&7 in the Discord server containing just &b{CODE}&7 to link your account.\n\n&7Discord Invite » &b{INVITE}";
const STILL_STARTING: &str = "&cCurrently unavailable to check link status because the server is still connecting to Discord.\n\nTry again in a minute.";

/// A kick text with its placeholders filled.
pub fn not_linked_text(template: &str, bot: &str, code: &str, invite: &str) -> String {
    template.replace("\\n", "\n").replace("{BOT}", bot).replace("{CODE}", code).replace("{INVITE}", invite)
}

/// The fake players (a mod's bots with no client) that joined: they get no join, leave or death notice.
static FAKES: std::sync::Mutex<std::collections::BTreeSet<u128>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// A fake player joined: nothing is announced for it, here or when it leaves.
pub fn fake_joined(uuid: u128) {
    FAKES.lock().unwrap_or_else(|e| e.into_inner()).insert(uuid);
}

/// Whether the player is a fake one that `fake_joined` heard.
pub fn is_fake(uuid: u128) -> bool {
    FAKES.lock().unwrap_or_else(|e| e.into_inner()).contains(&uuid)
}

/// A fake player left: true if it was one (and is forgotten).
pub fn fake_left(uuid: u128) -> bool {
    FAKES.lock().unwrap_or_else(|e| e.into_inner()).remove(&uuid)
}

/// A player joined; `message` is the join line as the game shows it. `silent`: the player has
/// `discordsrv.silentjoin` (checked by the glue), so nothing is announced.
pub fn player_join(uuid: u128, name: &str, message: &str, silent: bool) {
    let Some(sh) = shared() else { return };
    sh.join_notice(uuid, name, message, silent);
}

/// A player left; `message` is the leave line as the game shows it. `silent`: the player has
/// `discordsrv.silentquit`, so nothing is announced.
pub fn player_leave(uuid: u128, name: &str, message: &str, silent: bool) {
    if let Some(sh) = shared() {
        sh.join_notice_leave(uuid, name, message, silent);
    }
}

/// What other mods say about a player: the permissions provider's primary group (Vault's `getPrimaryGroup`)
/// and whether a vanish mod hides them (`PlayerUtil.isVanished`). Read on the server thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extra {
    pub primary_group: String,
    pub vanished: bool,
}

/// The players' [`Extra`]s and the world aliases (a Multiverse alias by dimension), refreshed about once a second.
/// A static of their own: the server thread sets them from inside the player snapshot, which holds the state lock.
#[derive(Debug, Default)]
struct Extras {
    players: HashMap<u128, Extra>,
    aliases: HashMap<String, String>,
}

static EXTRAS: Mutex<Option<Extras>> = Mutex::new(None);

fn extras<T>(f: impl FnOnce(&mut Extras) -> T) -> T {
    f(EXTRAS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Extras::default))
}

/// The server thread's view of the other mods, for the player list, the nickname synchronisation and the join notices.
pub fn set_player_extras(players: HashMap<u128, Extra>, aliases: HashMap<String, String>) {
    extras(|e| {
        e.players = players;
        e.aliases = aliases;
    });
}

/// A vanish mod hides the player (`PlayerUtil.isVanished`), as of the last snapshot.
pub fn is_vanished(uuid: u128) -> bool {
    extras(|e| e.players.get(&uuid).is_some_and(|x| x.vanished))
}

/// The player's primary group, empty when no provider publishes one.
pub fn primary_group(uuid: u128) -> String {
    extras(|e| e.players.get(&uuid).map(|x| x.primary_group.clone()).unwrap_or_default())
}

/// The alias a world mod gives the dimension; the world name when it gives none (`DiscordSRV#getWorldAlias`).
pub fn world_alias(dimension: &str, world: &str) -> String {
    extras(|e| e.aliases.get(dimension).filter(|a| !a.trim().is_empty()).cloned()).unwrap_or_else(|| world.to_string())
}

/// The groups of players for the synchronisation (`TickWork::resync`), read on the server thread:
/// the Discord roles are changed on the chat thread.
pub fn resync(players: HashMap<u128, sync::Groups>, direction: sync::Direction) {
    if let Some(sh) = shared() {
        sh.push_job(relay::Job::Resync { players, direction });
    }
}

/// The permissions provider published other groups for a player (DiscordSRV's LuckPerms hook, instant
/// group updates): their roles follow at once when the group synchronisation is on.
pub fn groups_changed(uuid: u128, groups: sync::Groups) {
    if !hook_enabled("LuckPerms-GroupUpdates") {
        return;
    }
    if let Some(sh) = shared()
        && sh.sync_enabled()
        && sh.lock_state().accounts.discord_of(uuid).is_some()
    {
        sh.push_job(relay::Job::Resync { players: HashMap::from([(uuid, groups)]), direction: sync::Direction::ToDiscord });
    }
}

/// The groups of `GroupRoleSynchronizationGroupsAndRolesToSync` (those with a real role) and whether
/// `GroupRoleSynchronizationEnableDenyPermission` is on: the server declares `discordsrv.sync.<group>` for each.
pub fn sync_group_names() -> (Vec<String>, bool) {
    let Some(sh) = shared() else { return (Vec::new(), false) };
    let c = &sh.config.chat;
    let mut names: Vec<String> = c
        .group_role_synchronization_groups_and_roles_to_sync
        .iter()
        .filter(|(g, r)| !g.trim().is_empty() && r.replace('0', "").trim().chars().any(|ch| ch.is_ascii_digit()))
        .map(|(g, _)| g.trim().to_string())
        .collect();
    names.sort();
    (names, c.group_role_synchronization_enable_deny_permission)
}

/// Whether a plugin hook is in use: not named in `DisabledPluginHooks` (case does not matter; `LuckPerms` also
/// switches off `LuckPerms-GroupUpdates` and `LuckPerms-Contexts`). Hooks are on while the bridge is off.
pub fn hook_enabled(name: &str) -> bool {
    let Some(sh) = shared() else { return true };
    let base = name.split('-').next().unwrap_or(name);
    !sh.config.disabled_plugin_hooks.iter().any(|d| d.trim().eq_ignore_ascii_case(name) || d.trim().eq_ignore_ascii_case(base))
}

/// The value of `%discordsrv_<identifier>%` for a player (`discordsrv:placeholder`): see [`placeholders`].
pub fn placeholder(uuid: Option<u128>, identifier: &str) -> Option<String> {
    if !hook_enabled("PlaceholderAPI") {
        return None;
    }
    placeholders::value(&shared()?, uuid, identifier)
}

/// The LuckPerms contexts for a player (`discordsrv:contexts`): key and value pairs, see [`placeholders::contexts`].
pub fn contexts(uuid: u128) -> Vec<(String, String)> {
    if !hook_enabled("LuckPerms-Contexts") {
        return Vec::new();
    }
    shared().map(|sh| placeholders::contexts(&sh, uuid)).unwrap_or_default()
}

/// Game ticks between the server thread's voice position samples, when the voice module runs.
pub fn voice_every() -> Option<u64> {
    shared().filter(|sh| sh.config.voice.enabled).map(|sh| sh.config.voice.tick_speed.max(1))
}

/// The server's players with their positions, for the voice module (every [`voice_every`] ticks).
pub fn voice_players(players: Vec<(u128, voice::VoicePlayer)>) {
    if let Some(sh) = shared()
        && sh.config.voice.enabled
    {
        sh.voice.lock().unwrap_or_else(|e| e.into_inner()).update_players(players);
    }
}

/// The UUIDs of every linked player (for `/discord resync`).
pub fn linked_uuids() -> Vec<u128> {
    shared().map(|sh| sh.linked_uuids()).unwrap_or_default()
}

/// A player was banned or pardoned on the server (`BanSynchronizationMinecraftToDiscord`): a ban
/// bans their linked Discord account.
pub fn minecraft_ban(uuid: u128, banned: bool) {
    if let Some(sh) = shared()
        && banned
    {
        sh.minecraft_ban(uuid);
    }
}

/// A player joined, so no ban stands on the server: the linked Discord account is unbanned
/// (`PlayerBanListener.onPlayerJoin`).
pub fn minecraft_unban_on_join(uuid: u128) {
    if let Some(sh) = shared() {
        sh.minecraft_unban(uuid);
    }
}

/// A player died; `message` is the death message (empty when unknown: nothing is sent).
pub fn player_death(uuid: u128, name: &str, dimension: &str, message: &str) {
    if let Some(sh) = shared() {
        sh.notify(relay::Notice::Death, uuid, name, dimension, message);
    }
}

/// A player made an advancement; `title` is its title as the game shows it.
pub fn player_advancement(uuid: u128, name: &str, dimension: &str, title: &str) {
    if let Some(sh) = shared() {
        sh.notify(relay::Notice::Advancement, uuid, name, dimension, title);
    }
}

/// A player's chat message (after the chat filter took it).
pub fn player_chat(uuid: u128, name: &str, message: &str) {
    let Some(sh) = shared() else { return };
    let chat = &sh.config.chat;
    if !chat.has_channels() || !chat.discord_chat_channel_minecraft_to_discord {
        return;
    }
    debugger::log("MINECRAFT_TO_DISCORD", &chat.debug, &format!("Chat from {name} queued for Discord"));
    let dimension = sh.lock_state().players.iter().find(|p| p.uuid == uuid).map(|p| p.dimension.clone()).unwrap_or_default();
    sh.push_job(relay::Job::GameChat { player: Player { uuid, name: name.to_string(), dimension }, message: message.to_string() });
}

/// Whether the bridge runs (the `/discord` command is registered only then).
pub fn is_running() -> bool {
    shared().is_some()
}

/// Whether the chat side runs (there is a chat channel).
pub fn chat_running() -> bool {
    shared().is_some_and(|sh| sh.config.chat.has_channels())
}

/// A player of the last snapshot (name and dimension), for hooks that only give a UUID.
pub fn known_player(uuid: u128) -> Option<Player> {
    shared().and_then(|sh| sh.lock_state().players.iter().find(|p| p.uuid == uuid).cloned())
}

/// The permission node and default operator level of a `/discord` sub-command word, None when it
/// is no sub-command.
pub fn command_permission(word: &str) -> Option<(&'static str, u8)> {
    commands::permission_of(word)
}

/// Id of the console as `/discord` sender (see `discord_command`).
pub const CONSOLE: u128 = commands::CONSOLE;

/// `/discord [link|linked|unlink]` typed by a player (`args` without the `discord`): the
/// `§`-coded answer lines, or None when the bridge isn't running (the command is not ours then).
pub fn discord_command(uuid: u128, name: &str, args: &str, has: &dyn Fn(&str, u8) -> bool) -> Option<Vec<String>> {
    match shared() {
        Some(sh) => Some(sh.discord_command(uuid, name, args, has)),
        None => commands::off_command(uuid, args, has),
    }
}

impl Shared {
    /// A bridge without threads or network, for tests of what it does with dispatches.
    #[cfg(test)]
    pub(crate) fn for_test(config: Config, dir: &Path) -> Shared {
        Shared {
            rest: rest::Rest::new("http://127.0.0.1:9", "test"),
            chat_filters: chat::ChatFilters::new(&config.chat).unwrap(),
            status_index: AtomicUsize::new(0),
            quiet_stop: AtomicBool::new(false),
            suppress_startup: AtomicBool::new(false),
            config,
            token: "test".into(),
            stop: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            tps: AtomicU32::new(20f32.to_bits()),
            commands: Mutex::new(VecDeque::new()),
            console_queue: Mutex::new(VecDeque::new()),
            replies: Mutex::new(Vec::new()),
            chat_console: Mutex::new(VecDeque::new()),
            usage_log_dir: dir.to_path_buf(),
            started: AtomicBool::new(false),
            last_tick_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            server: server::ServerInfo::parse(dir, "max-players=10\n"),
            version: "Horizon (MC: 26.3)".into(),
            state: Mutex::new(State {
                cache: guild::Cache::default(),
                accounts: linking::Accounts::load(dir.join("accounts.aof")),
                players: Vec::new(),
                new_players: HashSet::new(),
                ticks: 0,
            }),
            jobs: Mutex::new(VecDeque::new()),
            to_game: Mutex::new(TickWork::default()),
            alerts: Vec::new(),
            voice: Mutex::new(voice::Voice::default()),
            counts_wanted: AtomicBool::new(false),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_job(&self, job: relay::Job) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
    }

    /// A join: the first join when the player file was missing at login. Silent joins are not announced.
    fn join_notice(&self, uuid: u128, name: &str, message: &str, silent: bool) {
        let first = self.lock_state().new_players.remove(&uuid);
        if silent {
            return;
        }
        let kind = if first { relay::Notice::FirstJoin } else { relay::Notice::Join };
        self.notify(kind, uuid, name, "", message);
    }

    /// A leave; silent ones (`discordsrv.silentquit`) are not announced.
    fn join_notice_leave(&self, uuid: u128, name: &str, message: &str, silent: bool) {
        self.lock_state().new_players.remove(&uuid);
        if !silent {
            self.notify(relay::Notice::Leave, uuid, name, "", message);
        }
    }

    fn notify(&self, kind: relay::Notice, uuid: u128, name: &str, dimension: &str, text: &str) {
        if !self.config.chat.has_channels() {
            return;
        }
        self.push_job(relay::Job::Notify {
            kind,
            player: Player { uuid, name: name.to_string(), dimension: dimension.to_string() },
            text: text.to_string(),
        });
    }

    /// Lines for the console channel's next batch.
    fn reply(&self, lines: Vec<String>) {
        self.replies.lock().unwrap_or_else(|e| e.into_inner()).extend(lines);
    }

    /// A message in the console channel, through the preprocessor another mod set.
    fn console_command(&self, m: gateway::ChannelMessage) {
        let list: Vec<std::sync::Arc<Preprocessor>> =
            PREPROCESSORS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, f)| f.clone()).collect();
        let refs: Vec<&Preprocessor> = list.iter().map(|f| &**f).collect();
        self.console_command_with(m, &refs);
    }

    /// A message in the console channel: queued for the server thread when the preprocessor and
    /// the config let it run and the usage log took it.
    fn console_command_with(&self, m: gateway::ChannelMessage, pre: &[&Preprocessor]) {
        if m.bot && self.config.discord_console_channel_block_bots {
            return;
        }
        let mut command = m.content.trim().trim_start_matches('/').to_string();
        for f in pre {
            match f(&m.author_id, &command) {
                Preprocess::Run(c) => command = c.trim().trim_start_matches('/').to_string(),
                Preprocess::Refuse { reply } => {
                    // Posted at once, as `channel.sendMessage(reply).queue()`, not with the next log batch.
                    self.push_job(relay::Job::Post {
                        channel: self.config.discord_console_channel_id.clone(),
                        body: serde_json::json!({ "content": reply, "allowed_mentions": { "parse": [] } }),
                        expire: None,
                    });
                    return;
                }
            }
        }
        if !self.config.command_allowed(&command) {
            return;
        }
        let logged = self.log_usage(&m.author_id, &m.author, &m.content);
        if let Err(e) = &logged {
            log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't write the console usage log: {e}");
        }
        if logged.is_ok() || !self.config.cancel_console_command_if_logging_failed {
            if let Ok(mut q) = self.console_queue.lock() {
                q.push_back(command);
            }
        }
    }

    /// Writes `[<TimestampFormat> | ID <author id>] <author>: <message>` to the usage log
    /// (`DiscordConsoleChannelUsageLog`, `%date%` in `DateFormat`); a blank file name logs nothing.
    fn log_usage(&self, author_id: &str, author: &str, message: &str) -> std::io::Result<()> {
        let name = self.config.discord_console_channel_usage_log.trim();
        if name.is_empty() {
            return Ok(());
        }
        let c = &self.config.chat;
        let date_pattern = if c.date_format.is_empty() { "yyyy-MM-dd" } else { &c.date_format };
        let date = Local::now().format(&console::java_date_format(date_pattern)).to_string();
        let mut file =
            fs::OpenOptions::new().create(true).append(true).open(self.usage_log_dir.join(name.replace("%date%", &date)))?;
        writeln!(file, "[{} | ID {author_id}] {author}: {message}", self.date_now())
    }
}

struct CloseFeedOnDrop;

impl Drop for CloseFeedOnDrop {
    fn drop(&mut self) {
        LOG_FEED.close();
    }
}

/// Takes the logged lines, sends a batch every `DiscordConsoleChannelLogRefreshRateInSeconds` and the
/// topic every `ChannelTopicUpdaterRateInMinutes`; at the stop the rest and the shutdown topic.
fn console_loop(shared: &Shared, formatter: console::Formatter, version: &str) {
    let channel = shared.config.discord_console_channel_id.clone();
    let batch_every = Duration::from_secs(shared.config.discord_console_channel_log_refresh_rate_in_seconds.max(1));
    let topic_every = Duration::from_secs(shared.config.channel_topic_updater_rate_in_minutes.max(1) * 60);
    let mut pending: VecDeque<String> = VecDeque::new();
    let mut next_batch = Instant::now() + batch_every;
    let mut next_topic = Instant::now() + Duration::from_secs(5);
    let mut failing = false;
    loop {
        let stopping = shared.stop.load(Ordering::Relaxed);
        for line in LOG_FEED.drain() {
            if !shared.config.level_enabled(&line.level) {
                continue;
            }
            if let Some(text) = formatter.format(&line, &Local::now()) {
                pending.push_back(text);
            }
        }
        pending.extend(std::mem::take(&mut *shared.replies.lock().unwrap_or_else(|e| e.into_inner())));
        while pending.len() > MAX_PENDING_LINES {
            pending.pop_front();
        }
        if (stopping || Instant::now() >= next_batch) && !pending.is_empty() {
            let lines: Vec<String> = pending.drain(..).collect();
            for message in console::pack(&lines, shared.config.discord_console_channel_use_code_blocks) {
                match shared.rest.send_message(&channel, &message) {
                    Ok(()) => failing = false,
                    Err(e) => {
                        // Said once until it works again: this line is forwarded too.
                        if !failing {
                            log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't send console lines: {e}");
                        }
                        failing = true;
                    }
                }
            }
            next_batch = Instant::now() + batch_every;
        }
        let topic = &shared.config.channel_topic_updater_console_channel_topic_format;
        if !stopping && !topic.is_empty() && Instant::now() >= next_topic {
            let tps = f32::from_bits(shared.tps.load(Ordering::Relaxed));
            if let Err(e) = shared.rest.set_topic(&channel, &topic_text(topic, tps, version)) {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't set the console channel topic: {e}");
            }
            next_topic = Instant::now() + topic_every;
        }
        if stopping {
            let shutdown = &shared.config.channel_topic_updater_console_channel_topic_at_server_shutdown_format;
            if shared.config.channel_topic_updater_channel_topics_at_shutdown_enabled
                && !shutdown.is_empty()
                && !shared.quiet_stop.load(Ordering::Relaxed)
            {
                let _ = shared.rest.set_topic(&channel, &topic_text(shutdown, 0.0, version));
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The topic placeholders of messages.yml this port fills (see the README for the memory ones).
pub fn topic_text(format: &str, tps: f32, version: &str) -> String {
    let (used, free, max) = memory_gb();
    let now = Local::now();
    format
        .replace("%tps%", &format!("{tps:.1}"))
        .replace("%usedmemorygb%", &format!("{used:.1}"))
        .replace("%freememorygb%", &format!("{free:.1}"))
        .replace("%maxmemorygb%", &format!("{max:.1}"))
        .replace("%usedmemory%", &format!("{:.0}", used * 1024.0))
        .replace("%freememory%", &format!("{:.0}", free * 1024.0))
        .replace("%maxmemory%", &format!("{:.0}", max * 1024.0))
        .replace("%serverversion%", version)
        .replace("%date%", &now.format("%a %b %-d %H:%M:%S %Z %Y").to_string())
        .replace("%time%", &now.format("%H:%M:%S").to_string())
}

/// Used = this process's resident memory, free = the machine's available memory, max = its total
/// (a JVM's heap figures have no counterpart in a Rust server). Zeros where /proc is missing.
fn memory_gb() -> (f64, f64, f64) {
    let kb = |text: &str, key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let gb = 1024.0 * 1024.0;
    (kb(&status, "VmRSS:") / gb, kb(&meminfo, "MemAvailable:") / gb, kb(&meminfo, "MemTotal:") / gb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_find_online_players_by_at_name() {
        let names = vec!["Alex".to_string(), "Steve_2".to_string()];
        assert_eq!(mentioned("[§bDiscord§r] user » hi @alex, and @STEVE_2!", &names), vec!["Alex".to_string(), "Steve_2".to_string()]);
        assert!(mentioned("[Discord] user » alex without the at", &names).is_empty());
    }

    #[test]
    fn example_topics_fill_their_placeholders() {
        let c = Config::parse(config::DEFAULT_CONFIG).unwrap();
        let t = topic_text(&c.channel_topic_updater_console_channel_topic_format, 19.96, "Horizon (MC: 26.3)");
        assert!(t.starts_with("TPS: 20.0 | Mem: "), "{t}");
        assert!(t.ends_with("GB max | Horizon (MC: 26.3)"), "{t}");
        assert!(!t.contains('%'), "{t}");
        let s = topic_text(&c.channel_topic_updater_console_channel_topic_at_server_shutdown_format, 0.0, "V");
        assert_eq!(s, "Server is offline | V");
    }

    fn console_bridge(name: &str) -> (Shared, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hdsrv-lib-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut config = Config::parse(config::DEFAULT_CONFIG).unwrap();
        config.discord_console_channel_id = "500".into();
        config.discord_console_channel_blacklist_acts_as_whitelist = false;
        config.discord_console_channel_blacklisted_commands = vec!["stop".into()];
        (Shared::for_test(config, &dir), dir)
    }

    fn message(author_id: &str, content: &str) -> gateway::ChannelMessage {
        gateway::ChannelMessage { author: "user".into(), author_id: author_id.into(), bot: false, content: content.into() }
    }

    #[test]
    fn the_preprocessor_rewrites_and_refuses_and_the_command_list_still_applies() {
        let (sh, dir) = console_bridge("pre");
        let pre: Preprocessor = Box::new(|id: &str, command: &str| match command {
            "op me" if id == "11" => Preprocess::Run("op Alex".into()),
            "op me" => Preprocess::Refuse { reply: "not listed".into() },
            "sneaky" => Preprocess::Run("/stop".into()),
            other => Preprocess::Run(other.to_string()),
        });
        sh.console_command_with(message("11", "/op me"), &[&pre]);
        sh.console_command_with(message("12", "op me"), &[&pre]);
        sh.console_command_with(message("12", "sneaky"), &[&pre]);
        sh.console_command_with(message("12", "list"), &[&pre]);
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["op Alex".to_string(), "list".to_string()]);
        // the refusal goes out as its own message, not into the next log batch
        assert!(sh.replies.lock().unwrap().is_empty());
        let jobs = sh.jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        match &jobs[0] {
            relay::Job::Post { channel, body, .. } => {
                assert_eq!(channel, "500");
                assert_eq!(body["content"], "not listed");
            }
            _ => panic!("expected a post"),
        }
        drop(jobs);
        assert!(sh.commands.lock().unwrap().is_empty());
        // Without a preprocessor the message runs as it came.
        sh.console_command_with(message("12", "op me"), &[]);
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["op me".to_string()]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// DiscordSRV `api.subscribe` takes any number of listeners and `unsubscribe` removes one.
    #[test]
    fn several_preprocessors_chain_and_one_can_be_removed() {
        let (sh, dir) = console_bridge("chain");
        let a: Preprocessor = Box::new(|_: &str, c: &str| Preprocess::Run(if c == "x" { "list".into() } else { c.into() }));
        let b: Preprocessor = Box::new(|_: &str, c: &str| {
            if c == "list" { Preprocess::Refuse { reply: "no".into() } } else { Preprocess::Run(c.into()) }
        });
        sh.console_command_with(message("1", "x"), &[&a, &b]);
        assert!(sh.console_queue.lock().unwrap().is_empty());
        assert_eq!(sh.jobs.lock().unwrap().len(), 1);
        sh.console_command_with(message("1", "x"), &[&a]);
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["list".to_string()]);
        let h1 = add_console_preprocessor(Box::new(|_: &str, c: &str| Preprocess::Run(c.into())));
        let h2 = add_console_preprocessor(Box::new(|_: &str, c: &str| Preprocess::Run(c.into())));
        assert!(remove_console_preprocessor(h1));
        assert!(!remove_console_preprocessor(h1));
        assert!(remove_console_preprocessor(h2));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_not_linked_text_fills_bot_code_and_invite() {
        let text = not_linked_text("Link {CODE} to {BOT} at {INVITE}", "Bridge", "1234", "https://example.invalid/x");
        assert_eq!(text, "Link 1234 to Bridge at https://example.invalid/x");
        assert!(NOT_LINKED.contains("{CODE}") && NOT_LINKED.contains("{BOT}") && NOT_LINKED.contains("{INVITE}"));
    }

    #[test]
    fn silent_joins_and_quits_are_not_announced() {
        let mut c = Config::parse(config::DEFAULT_CONFIG).unwrap();
        c.chat.channels = vec![("global".into(), "600".into())];
        let dir = std::env::temp_dir().join(format!("hdsrv-lib-silent-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let sh = Shared::for_test(c, &dir);
        sh.join_notice(1, "a", "joined", true);
        sh.join_notice_leave(1, "a", "left", true);
        assert!(sh.jobs.lock().unwrap().is_empty());
        sh.join_notice(2, "b", "joined", false);
        sh.join_notice_leave(2, "b", "left", false);
        assert_eq!(sh.jobs.lock().unwrap().len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn command_answers_lose_colours_and_silence_says_whether_it_ran() {
        assert_eq!(
            output_lines("list", true, vec!["§eThere are 0 of a max of 10 players online:\n".into(), "".into()]),
            vec!["There are 0 of a max of 10 players online:".to_string()]
        );
        assert_eq!(output_lines("time query daytime", true, vec![]), vec!["> time query daytime: ran, no output".to_string()]);
        assert_eq!(output_lines("nope", false, vec![]), vec!["> nope: did not run".to_string()]);
    }
}
