//! DiscordSRV for a Horizon dedicated server: the console channel (the server's log streamed
//! to a Discord channel, commands typed there run by the console), the chat channels (game chat
//! both ways, join/leave/death/advancement notifications, start/stop messages, the player list,
//! canned responses, topics and channel names, the watchdog), account linking, and the bot's
//! presence. Everything that touches the network runs on threads of its own; the server thread
//! hands over its TPS, its players and the game's events, and takes queued commands and lines.
pub mod chat;
pub mod commands;
pub mod config;
pub mod console;
pub mod embed;
pub mod feed;
pub mod format;
pub mod gateway;
pub mod guild;
pub mod linking;
pub mod relay;
pub mod rest;
pub mod server;
pub mod ws;

use std::path::PathBuf;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;

use config::Config;
pub use server::Player;

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
    state: Mutex<State>,
    jobs: Mutex<VecDeque<relay::Job>>,
    to_game: Mutex<TickWork>,
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

static PREPROCESSOR: Mutex<Option<Preprocessor>> = Mutex::new(None);

/// Lets another mod look at each console channel message before it runs (DiscordSRV's console
/// command pre-process event): `f(author_id, command)` gets the author's Discord user ID and the
/// command without its leading slash, after the bot check and before the command list, so a
/// command it returns must still pass the list. One preprocessor; a later call replaces it. It
/// runs on the bridge's gateway thread and must not call back into this function.
pub fn set_console_preprocessor(f: Preprocessor) {
    *PREPROCESSOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
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
    if !start_inner(&mut slot, config_dir, server_version) {
        LOG_FEED.close();
    }
}

/// [`start`]; false when the console channel will not run, so the log feed can close.
fn start_inner(slot: &mut Option<Running>, config_dir: &Path, server_version: String) -> bool {
    let dir = config_dir.join("discordsrv");
    let path = dir.join("config.toml");
    if !path.exists() {
        let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(&path, config::DEFAULT_CONFIG));
    }
    let config = match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Config::parse(&t)) {
        Ok(c) => c,
        Err(e) => {
            log::error!(target: "horizon_discordsrv", "[DiscordSRV] {}: {e}", path.display());
            return false;
        }
    };
    let console_on = !config.discord_console_channel_id.trim().is_empty();
    if !console_on && !config.chat.has_channels() {
        log::info!(target: "horizon_discordsrv", "[DiscordSRV] No console or chat channel set in {}: not connecting", path.display());
        return false;
    }
    let token = match fs::read_to_string(dir.join(&config.bot_token_file)) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            log::error!(target: "horizon_discordsrv", "[DiscordSRV] No bot token in {}: not connecting", config.bot_token_file);
            return false;
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
    let shared = Arc::new(Shared {
        rest: rest::Rest::new(&config.api_base_url, &token),
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
        started: AtomicBool::new(false),
        last_tick_ms: AtomicU64::new(0),
        epoch: Instant::now(),
        server: server::ServerInfo::load(&home),
        version: server_version,
        chat_filters,
        state: Mutex::new(State {
            cache: guild::Cache::default(),
            accounts,
            players: Vec::new(),
            new_players: HashSet::new(),
            ticks: 0,
        }),
        jobs: Mutex::new(VecDeque::new()),
        to_game: Mutex::new(TickWork::default()),
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

/// A player joined; `message` is the join line as the game shows it.
pub fn player_join(uuid: u128, name: &str, message: &str) {
    let Some(sh) = shared() else { return };
    let first = sh.lock_state().new_players.remove(&uuid);
    let kind = if first { relay::Notice::FirstJoin } else { relay::Notice::Join };
    sh.notify(kind, uuid, name, "", message);
}

/// A player left; `message` is the leave line as the game shows it.
pub fn player_leave(uuid: u128, name: &str, message: &str) {
    if let Some(sh) = shared() {
        sh.lock_state().new_players.remove(&uuid);
        sh.notify(relay::Notice::Leave, uuid, name, "", message);
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

/// `/discord [link|linked|unlink]` typed by a player (`args` without the `discord`): the
/// `§`-coded answer lines, or None when the bridge isn't running (the command is not ours then).
pub fn discord_command(uuid: u128, name: &str, args: &str, has: &dyn Fn(&str, u8) -> bool) -> Option<Vec<String>> {
    shared().map(|sh| sh.discord_command(uuid, name, args, has))
}

impl Shared {
    /// A bridge without threads or network, for tests of what it does with dispatches.
    #[cfg(test)]
    pub(crate) fn for_test(config: Config, dir: &Path) -> Shared {
        Shared {
            rest: rest::Rest::new("http://127.0.0.1:9", "test"),
            chat_filters: chat::ChatFilters::new(&config.chat).unwrap(),
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
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_job(&self, job: relay::Job) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
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
        let pre = PREPROCESSOR.lock().unwrap_or_else(|e| e.into_inner());
        self.console_command_with(m, pre.as_ref());
    }

    /// A message in the console channel: queued for the server thread when the preprocessor and
    /// the config let it run and the usage log took it.
    fn console_command_with(&self, m: gateway::ChannelMessage, pre: Option<&Preprocessor>) {
        if m.bot && self.config.discord_console_channel_block_bots {
            return;
        }
        let mut command = m.content.trim().trim_start_matches('/').to_string();
        if let Some(f) = pre {
            match f(&m.author_id, &command) {
                Preprocess::Run(c) => command = c.trim().trim_start_matches('/').to_string(),
                Preprocess::Refuse { reply } => {
                    self.reply(vec![reply]);
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
            if shared.config.channel_topic_updater_channel_topics_at_shutdown_enabled && !shutdown.is_empty() {
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
        sh.console_command_with(message("11", "/op me"), Some(&pre));
        sh.console_command_with(message("12", "op me"), Some(&pre));
        sh.console_command_with(message("12", "sneaky"), Some(&pre));
        sh.console_command_with(message("12", "list"), Some(&pre));
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["op Alex".to_string(), "list".to_string()]);
        assert_eq!(*sh.replies.lock().unwrap(), vec!["not listed".to_string()]);
        assert!(sh.commands.lock().unwrap().is_empty());
        // Without a preprocessor the message runs as it came.
        sh.console_command_with(message("12", "op me"), None);
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["op me".to_string()]);
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
