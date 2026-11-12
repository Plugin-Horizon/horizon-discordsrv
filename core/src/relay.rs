//! The chat side's work: what the gateway hands over (chat channel messages, PMs with link codes,
//! the player list command, canned responses), the in-game `/discord` command, and the
//! `DiscordSRV - Chat` thread that posts chat and notifications, the start/stop messages, the
//! chat channel topic, the channel names and the watchdog's alarm.
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use serde_json::{Value, json};

use crate::chat::{self, FromDiscord, GameChat};
use crate::console::java_date_format;
use crate::format::{escape_markdown, fill, strip_codes, translate_ampersand, truncate};
use crate::linking::CodeResult;
use crate::server::{Player, uuid_string};
use crate::{Shared, State, embed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Join,
    FirstJoin,
    Leave,
    Death,
    Advancement,
}

impl Notice {
    /// The game channel name of `Channels` this kind goes to.
    fn channel(self) -> &'static str {
        match self {
            Notice::Join | Notice::FirstJoin => "join",
            Notice::Leave => "leave",
            Notice::Death => "deaths",
            Notice::Advancement => "awards",
        }
    }
}

/// Work for the chat thread (everything that waits on Discord's REST API).
#[derive(Debug)]
pub enum Job {
    /// A message body to a channel; deleted again after `expire` (with the request message).
    Post { channel: String, body: Value, expire: Option<(Duration, Option<String>)> },
    /// A notification of messages.yml: join, leave, death (`text` = death message),
    /// advancement (`text` = its title).
    Notify { kind: Notice, player: Player, text: String },
    /// A player's chat message.
    GameChat { player: Player, message: String },
    /// A PM to a Discord user.
    Dm { user: String, content: String },
    /// Accounts linked: role and console commands.
    Linked { discord: String, discord_name: String, uuid: u128, name: String },
    /// Accounts unlinked: role taken and console commands.
    Unlinked { discord: String, uuid: u128, name: String },
    /// A message deleted, after `after` (zero: now).
    Delete { channel: String, message: String, after: Duration },
    /// Reactions added to a message, in this order.
    React { channel: String, message: String, emoji: Vec<&'static str> },
}

fn no_mentions(content: &str) -> Value {
    json!({ "content": content, "allowed_mentions": { "parse": [] } })
}

impl Shared {
    /// Every gateway dispatch: the guild cache learns from it, and messages go where they belong.
    pub(crate) fn dispatch(&self, event: &str, d: &Value) {
        let mut st = self.lock_state();
        st.cache.apply(event, d);
        if event != "MESSAGE_CREATE" {
            return;
        }
        let channel = d["channel_id"].as_str().unwrap_or_default();
        let console = self.config.discord_console_channel_id.trim();
        if !console.is_empty() && channel == console {
            drop(st);
            if let Some(m) = crate::gateway::console_message(d, console) {
                self.console_command(m);
            }
            return;
        }
        let author = d["author"]["id"].as_str().unwrap_or_default();
        if author.is_empty() || author == st.cache.bot_id {
            return;
        }
        let c = &self.config.chat;
        let in_guild = d.get("guild_id").is_some_and(|g| !g.is_null());
        let bot = d["author"]["bot"].as_bool().unwrap_or(false);
        if !in_guild {
            // DiscordAccountLinkListener: a PM to the bot is a link code.
            if c.minecraft_discord_account_linked_use_pm && !bot {
                self.link_code(&mut st, d, false);
            }
            return;
        }
        let webhook = d.get("webhook_id").is_some_and(|w| !w.is_null());
        if webhook && c.discord_chat_channel_block_webhooks {
            return;
        }
        // The link channel takes codes from people (and nothing else), whatever UsePM says.
        if c.own_channel("link") == Some(channel) {
            if !bot {
                self.link_code(&mut st, d, true);
            }
            return;
        }
        let content = d["content"].as_str().unwrap_or_default();
        // Canned responses answer in any channel the bot reads; the message goes no further. The
        // trigger only has to start the message, case aside. (Several matches: the longest.)
        let lower = content.to_lowercase();
        let canned = c
            .discord_canned_responses
            .iter()
            .filter(|(k, _)| !k.is_empty() && lower.starts_with(&k.to_lowercase()))
            .max_by_key(|(k, _)| k.len());
        if let Some((_, answer)) = canned {
            let text = strip_codes(&self.server_text(answer, &st.players));
            if !text.trim().is_empty() {
                let body = json!({ "content": truncate(&text, crate::console::MAX_MESSAGE), "allowed_mentions": chat::allowed_mentions(c) });
                self.push_job(Job::Post { channel: channel.to_string(), body, expire: None });
            }
            return;
        }
        if !c.is_chat_channel(channel) {
            return;
        }
        let has_extras = ["attachments", "sticker_items"].iter().any(|k| d[*k].as_array().is_some_and(|a| !a.is_empty()));
        if content.trim().is_empty() && !has_extras {
            return;
        }
        let list = c.discord_chat_channel_list_command_message.as_str();
        if c.discord_chat_channel_list_command_enabled && content.trim().eq_ignore_ascii_case(list) {
            let body = json!({ "content": self.player_list(&st.players), "allowed_mentions": chat::allowed_mentions(c) });
            let expire = (c.discord_chat_channel_list_command_expiration > 0).then(|| {
                let request = c.discord_chat_channel_list_command_expiration_delete_request;
                let id = d["id"].as_str().filter(|_| request).map(str::to_string);
                (Duration::from_secs(c.discord_chat_channel_list_command_expiration), id)
            });
            drop(st);
            self.push_job(Job::Post { channel: channel.to_string(), body, expire });
            return;
        }
        if self.chat_console_command(&st, d, content) {
            return;
        }
        if !c.discord_chat_channel_discord_to_minecraft {
            return;
        }
        let message_id = d["id"].as_str().unwrap_or_default().to_string();
        let outcome = {
            let st: &State = &st;
            chat::discord_to_minecraft(c, &self.chat_filters, &st.cache, d, |id| st.accounts.uuid_of(id).is_some())
        };
        drop(st);
        let channel = channel.to_string();
        match outcome {
            // The server logs every broadcast, whatever BroadcastDiscordMessagesToConsole says (GAPS.md).
            FromDiscord::Broadcast { lines, truncated } => {
                self.to_game.lock().unwrap_or_else(|e| e.into_inner()).broadcasts.extend(lines);
                if truncated {
                    self.push_job(Job::React { channel, message: message_id, emoji: vec!["\u{1F4AC}", "\u{2757}"] });
                }
            }
            FromDiscord::Rejected => self.push_job(Job::React { channel, message: message_id, emoji: vec!["\u{274C}"] }),
            FromDiscord::LinkRequired(text) => {
                if let Some(content) = text {
                    self.push_job(Job::Dm { user: author.to_string(), content });
                }
                self.push_job(Job::Delete { channel, message: message_id, after: Duration::ZERO });
            }
            FromDiscord::Ignore => {}
        }
    }

    /// Whether a member has one of the roles (names without case, or IDs); `@everyone` is everybody.
    fn has_role(&self, st: &State, d: &Value, wanted: &[&String]) -> bool {
        if wanted.iter().any(|w| w.as_str() == "@everyone") {
            return true;
        }
        let ids: Vec<String> = d["member"]["roles"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let channel = d["channel_id"].as_str().unwrap_or_default();
        let Some(guild) = st.cache.guild_of_channel(channel) else { return false };
        st.cache
            .sorted_roles(guild, &ids)
            .iter()
            .any(|r| wanted.iter().any(|w| w.eq_ignore_ascii_case(&r.name) || w.as_str() == r.id))
    }

    /// `!c <command>` in a chat channel (DiscordSRV's `processConsoleCommand`): true when the
    /// message was a console command (allowed or not), so it is not chat.
    fn chat_console_command(&self, st: &State, d: &Value, content: &str) -> bool {
        let c = &self.config.chat;
        let prefix = c.discord_chat_channel_console_command_prefix.as_str();
        if !c.discord_chat_channel_console_command_enabled || prefix.is_empty() {
            return false;
        }
        let head: String = content.chars().take(prefix.chars().count()).collect();
        if !head.eq_ignore_ascii_case(prefix) {
            return false;
        }
        let command = content.chars().skip(prefix.chars().count()).collect::<String>().trim().to_string();
        let webhook = d.get("webhook_id").is_some_and(|w| !w.is_null());
        let author = d["author"]["id"].as_str().unwrap_or_default();
        let user = d["author"]["username"].as_str().unwrap_or_default();
        let allowed_roles: Vec<&String> = c
            .discord_chat_channel_console_command_roles_allowed
            .iter()
            .chain(&c.discord_chat_channel_console_command_whitelist_bypass_roles)
            .collect();
        let refuse = |error: &str| {
            if c.discord_chat_channel_console_command_notify_errors {
                let text = fill(&c.discord_chat_channel_console_command_notify_errors_format, &[("user", user), ("error", error)]);
                self.push_job(Job::Dm { user: author.to_string(), content: text });
            }
        };
        if !(webhook || self.has_role(st, d, &allowed_roles)) {
            refuse("no permission");
            return true;
        }
        let bypass: Vec<&String> = c.discord_chat_channel_console_command_whitelist_bypass_roles.iter().collect();
        if !self.has_role(st, d, &bypass) {
            let requested = command.split(' ').next().unwrap_or_default();
            let listed = c.discord_chat_channel_console_command_whitelist.iter().any(|w| w == requested);
            if listed == c.discord_chat_channel_console_command_whitelist_acts_as_blacklist {
                refuse("command is not able to be used");
                return true;
            }
        }
        let logged = self.log_usage(author, user, content);
        if let Err(e) = &logged {
            log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't write the console usage log: {e}");
            if self.config.cancel_console_command_if_logging_failed {
                return true;
            }
        }
        let channel = d["channel_id"].as_str().unwrap_or_default().to_string();
        let expiration = c.discord_chat_channel_console_command_expiration;
        if expiration > 0
            && c.discord_chat_channel_console_command_expiration_delete_request
            && let Some(id) = d["id"].as_str()
        {
            self.push_job(Job::Delete { channel: channel.clone(), message: id.to_string(), after: Duration::from_secs(expiration) });
        }
        self.chat_console.lock().unwrap_or_else(|e| e.into_inner()).push_back((command.clone(), channel));
        self.console_queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(command);
        true
    }

    /// The answer of a command typed in a chat channel: its lines, markdown escaped, in as few
    /// messages as fit, deleted after `DiscordChatChannelConsoleCommandExpiration` seconds.
    pub(crate) fn chat_console_answer(&self, channel: &str, output: &[String]) {
        let c = &self.config.chat;
        let lines: Vec<String> = output.iter().flat_map(|l| l.split('\n')).map(crate::console::strip_colors).collect();
        let expire = (c.discord_chat_channel_console_command_expiration > 0)
            .then(|| (Duration::from_secs(c.discord_chat_channel_console_command_expiration), None));
        let mut body = String::new();
        let send = |text: &mut String| {
            if !text.trim().is_empty() {
                let content = strip_codes(text);
                self.push_job(Job::Post {
                    channel: channel.to_string(),
                    body: json!({ "content": content, "allowed_mentions": chat::allowed_mentions(c) }),
                    expire: expire.clone(),
                });
            }
            text.clear();
        };
        for line in lines {
            let line = escape_markdown(&line);
            if !body.is_empty() && body.chars().count() + 1 + line.chars().count() > 1998 {
                send(&mut body);
            }
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&line);
        }
        send(&mut body);
    }

    /// A message to the bot read as a link code; the answer goes back where it came from (as a
    /// reply). In the link channel the answer and the code are deleted after
    /// `MinecraftDiscordAccountLinkedMessageDeleteSeconds`.
    fn link_code(&self, st: &mut State, d: &Value, in_channel: bool) {
        let c = &self.config.chat;
        let author = d["author"]["id"].as_str().unwrap_or_default().to_string();
        let username = d["author"]["username"].as_str().unwrap_or_default().to_string();
        let content = d["content"].as_str().unwrap_or_default();
        let digits: String = content.chars().filter(char::is_ascii_digit).collect();
        let mention = format!("<@{author}>");
        let relink = c.minecraft_discord_account_linked_allow_relink_by_sending_a_new_code;
        let answer = match st.accounts.redeem(&author, content, relink) {
            Ok(CodeResult::Invalid) => fill(&c.invalid_code, &[("code", &digits), ("mention", &mention)]),
            Ok(CodeResult::Unknown) => fill(&c.unknown_code, &[("code", &digits), ("mention", &mention)]),
            Ok(CodeResult::AlreadyLinked(uuid)) => {
                let name = crate::commands::known_name(st, &self.server, uuid).unwrap_or_else(|| "<Unknown>".to_string());
                fill(&c.discord_account_already_linked, &[("uuid", &uuid_string(uuid)), ("username", &name), ("mention", &mention)])
            }
            Ok(CodeResult::Linked { uuid, name, replaced }) => {
                if let Some(old) = replaced {
                    let old_name = crate::commands::known_name(st, &self.server, old).unwrap_or_else(|| uuid_string(old));
                    self.push_job(Job::Unlinked { discord: author.clone(), uuid: old, name: old_name });
                }
                let line = fill(&c.minecraft_account_linked, &[("username", &username), ("id", &author)]);
                self.to_game.lock().unwrap_or_else(|e| e.into_inner()).tells.push((uuid, translate_ampersand(&line)));
                self.push_job(Job::Linked { discord: author.clone(), discord_name: username, uuid, name: name.clone() });
                fill(
                    &c.discord_account_linked,
                    &[("name", &name), ("displayname", &name), ("uuid", &uuid_string(uuid)), ("mention", &mention)],
                )
            }
            Err(e) => {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't save the linked accounts: {e}");
                return;
            }
        };
        let channel = d["channel_id"].as_str().unwrap_or_default().to_string();
        let mut body = json!({ "content": answer, "allowed_mentions": chat::allowed_mentions(c) });
        if let Some(id) = d["id"].as_str() {
            body["message_reference"] = json!({ "message_id": id, "fail_if_not_exists": false });
        }
        let delete = c.minecraft_discord_account_linked_message_delete_seconds;
        let expire = (in_channel && delete > 0).then(|| (Duration::from_secs(delete), d["id"].as_str().map(str::to_string)));
        self.push_job(Job::Post { channel, body, expire });
    }

    /// The player list command's answer: the heading with `n/max`, then the players sorted, in a
    /// code block that is cut at 1996 characters.
    fn player_list(&self, players: &[Player]) -> String {
        let c = &self.config.chat;
        if players.is_empty() {
            return strip_codes(&c.discord_chat_channel_list_command_format_no_online_players);
        }
        let count = format!("{}/{}", players.len(), self.server.max_players);
        let head = fill(&c.discord_chat_channel_list_command_format_online_players, &[("playercount", &count)]);
        let mut formatted: Vec<String> = players
            .iter()
            .map(|p| {
                let world = self.server.world_name(&p.dimension);
                let name = strip_codes(&p.name);
                fill(
                    &c.discord_chat_channel_list_command_player_format,
                    &[("displayname", &name), ("username", &p.name), ("primarygroup", ""), ("worldalias", &world), ("world", &world)],
                )
            })
            .collect();
        formatted.sort();
        let mut message = format!("{head}\n```\n{}", formatted.join(&c.discord_chat_channel_list_command_all_players_separator));
        if message.chars().count() > 1996 {
            message = message.chars().take(1993).collect::<String>() + "...";
        }
        strip_codes(&(message + "\n```"))
    }

    /// `%date%` as `TimestampFormat` says.
    pub(crate) fn date_now(&self) -> String {
        let pattern = if self.config.chat.timestamp_format.is_empty() {
            "EEE, d. MMM yyyy HH:mm:ss z"
        } else {
            &self.config.chat.timestamp_format
        };
        Local::now().format(&java_date_format(pattern)).to_string()
    }

    /// The server placeholders of topics, channel names and canned responses.
    pub(crate) fn server_text(&self, format: &str, players: &[Player]) -> String {
        let uptime = self.epoch.elapsed().as_secs();
        let tps = f32::from_bits(self.tps.load(Ordering::Relaxed));
        let filled = fill(
            format,
            &[
                ("playercount", &players.len().to_string()),
                ("playermax", &self.server.max_players.to_string()),
                ("totalplayers", &self.server.total_players().to_string()),
                ("uptimemins", &(uptime / 60).to_string()),
                ("uptimehours", &(uptime / 3600).to_string()),
                ("motd", &strip_codes(&self.server.motd)),
                ("date", &self.date_now()),
                ("timestamp", &Utc::now().timestamp().to_string()),
            ],
        );
        crate::topic_text(&filled, tps, &self.version)
    }

    fn post_logged(&self, channel: &str, body: &Value, what: &str) -> Option<String> {
        match self.rest.post(channel, body) {
            Ok(id) => Some(id),
            Err(e) => {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't send the {what}: {e}");
                None
            }
        }
    }

    fn run_job(&self, job: Job, expiring: &mut Vec<(Instant, String, String)>) {
        let c = &self.config.chat;
        match job {
            Job::Post { channel, body, expire } => {
                if let Some(id) = self.post_logged(&channel, &body, "message")
                    && let Some((after, request)) = expire
                {
                    let at = Instant::now() + after;
                    expiring.push((at, channel.clone(), id));
                    if let Some(r) = request {
                        expiring.push((at, channel, r));
                    }
                }
            }
            Job::Notify { kind, player, text } => {
                let Some(channel) = c.channel_for(kind.channel()) else { return };
                let template = match kind {
                    Notice::Join => &c.minecraft_player_join_message,
                    Notice::FirstJoin => &c.minecraft_player_first_join_message,
                    Notice::Leave => &c.minecraft_player_leave_message,
                    Notice::Death => &c.minecraft_player_death_message,
                    Notice::Advancement => &c.minecraft_player_achievement_message,
                };
                if kind == Notice::Death && text.trim().is_empty() {
                    return;
                }
                let (bot_name, bot_avatar) = {
                    let st = self.lock_state();
                    (st.cache.bot_name.clone(), st.cache.bot_avatar.clone())
                };
                let name = strip_codes(&player.name);
                let escaped = escape_markdown(&name);
                let avatar = embed::avatar_url(&c.avatar_url, &name, player.uuid, 128);
                let world = self.server.world_name(&player.dimension);
                let text = strip_codes(&text);
                let date = self.date_now();
                let pairs = [
                    ("displaynamenoescapes", name.as_str()),
                    ("usernamenoescapes", &name),
                    ("displayname", &escaped),
                    ("username", &escaped),
                    ("date", &date),
                    ("embedavatarurl", &avatar),
                    ("botavatarurl", &bot_avatar),
                    ("botname", &bot_name),
                    ("message", &text),
                    ("deathmessage", &text),
                    ("achievement", &text),
                    ("world", &world),
                ];
                if let Some(mut body) = embed::render(template, &pairs, Utc::now().timestamp()) {
                    body["allowed_mentions"] = json!({ "parse": [] });
                    self.post_logged(channel, &body, "notification");
                }
            }
            Job::GameChat { player, message } => {
                let Some(channel) = c.channel_for("global") else { return };
                let world = self.server.world_name(&player.dimension);
                let date = self.date_now();
                let content = {
                    let st = self.lock_state();
                    let m = GameChat {
                        username: &player.name,
                        // No display name helper (GAPS.md): the name.
                        display_name: &player.name,
                        message: &message,
                        // No permissions/groups API between mods (GAPS.md).
                        primary_group: "",
                        world: &world,
                        date: &date,
                        channel: "global",
                    };
                    chat::minecraft_to_discord(c, &self.chat_filters, &st.cache, &m)
                };
                if let Some(content) = content {
                    let body = json!({ "content": content, "allowed_mentions": chat::allowed_mentions(c) });
                    self.post_logged(channel, &body, "chat message");
                }
            }
            Job::Dm { user, content } => match self.rest.dm_channel(&user) {
                Ok(dm) => {
                    self.post_logged(&dm, &no_mentions(&content), "PM");
                }
                Err(e) => log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't open a PM: {e}"),
            },
            Job::Delete { channel, message, after } => {
                if after.is_zero() {
                    if let Err(e) = self.rest.delete_message(&channel, &message) {
                        log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't delete a message: {e}");
                    }
                } else {
                    expiring.push((Instant::now() + after, channel, message));
                }
            }
            Job::React { channel, message, emoji } => {
                for e in emoji {
                    if let Err(err) = self.rest.add_reaction(&channel, &message, e) {
                        log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't add a reaction: {err}");
                        break;
                    }
                }
            }
            Job::Linked { discord, discord_name, uuid, name } => {
                self.role(&discord, true);
                self.link_commands(&c.minecraft_discord_account_linked_console_commands, &discord, &discord_name, uuid, &name);
            }
            Job::Unlinked { discord, uuid, name } => {
                self.role(&discord, false);
                let discord_name = self.lock_state().cache.members.get(&discord).map(|m| m.username.clone()).unwrap_or_default();
                self.link_commands(&c.minecraft_discord_account_unlinked_console_commands, &discord, &discord_name, uuid, &name);
            }
        }
    }

    /// `MinecraftDiscordAccountLinkedRoleNameToAddUserTo` given or taken.
    fn role(&self, user: &str, add: bool) {
        let role_name = self.config.chat.minecraft_discord_account_linked_role_name_to_add_user_to.trim();
        if role_name.is_empty() {
            return;
        }
        let target = {
            let st = self.lock_state();
            let main = self.config.chat.channel_for("global").unwrap_or_default();
            st.cache
                .guild_of_channel(main)
                .and_then(|g| st.cache.find_role(g, role_name).map(|r| (g.id.clone(), r.id.clone())))
        };
        let Some((guild, role)) = target else { return };
        let result = if add { self.rest.add_role(&guild, user, &role) } else { self.rest.remove_role(&guild, user, &role) };
        if let Err(e) = result {
            log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't change the linked role: {e}");
        }
    }

    fn link_commands(&self, commands: &[String], discord: &str, discord_name: &str, uuid: u128, name: &str) {
        let uuid = uuid_string(uuid);
        let mut q = self.commands.lock().unwrap_or_else(|e| e.into_inner());
        for command in commands.iter().filter(|c| !c.trim().is_empty()) {
            q.push_back(fill(
                command.trim().trim_start_matches('/'),
                &[
                    ("minecraftplayername", name),
                    ("minecraftdisplayname", name),
                    ("minecraftuuid", &uuid),
                    ("discordid", discord),
                    ("discordname", discord_name),
                    ("discorddisplayname", discord_name),
                ],
            ));
        }
    }

    fn drain_jobs(&self, expiring: &mut Vec<(Instant, String, String)>) {
        loop {
            let job = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            match job {
                Some(j) => self.run_job(j, expiring),
                None => return,
            }
        }
    }
}

/// The chat thread: jobs as they come, the startup message after the first tick, the chat
/// channel topic and the channel names on their timers, the watchdog, and at the stop the
/// shutdown message and formats.
pub fn chat_loop(shared: &Shared) {
    let c = &shared.config.chat;
    let main = c.channel_for("global").map(str::to_string);
    let mut startup_sent = false;
    let mut expiring: Vec<(Instant, String, String)> = Vec::new();
    let topic_every = Duration::from_secs(shared.config.channel_topic_updater_rate_in_minutes.max(1) * 60);
    let mut next_topic = Instant::now() + Duration::from_secs(5);
    let mut next_names: Vec<Instant> = c.channel_updater.iter().map(|_| Instant::now() + Duration::from_secs(5)).collect();
    let mut watchdog_fired = false;
    loop {
        let stopping = shared.stop.load(Ordering::Relaxed);
        shared.drain_jobs(&mut expiring);
        let now = Instant::now();
        let started = shared.started.load(Ordering::Relaxed);

        if started && !startup_sent && !stopping {
            startup_sent = true;
            let text = c.discord_chat_channel_server_startup_message.trim();
            if let Some(channel) = c.channel_for("status").filter(|_| !text.is_empty()) {
                shared.post_logged(channel, &no_mentions(text), "startup message");
            }
        }

        let (due, later): (Vec<_>, Vec<_>) = expiring.drain(..).partition(|(at, _, _)| *at <= now);
        expiring = later;
        for (_, channel, id) in due {
            let _ = shared.rest.delete_message(&channel, &id);
        }

        let players = shared.lock_state().players.clone();
        let topic = &c.channel_topic_updater_chat_channel_topic_format;
        if !stopping && now >= next_topic {
            if let Some(channel) = main.as_deref().filter(|_| !topic.is_empty())
                && let Err(e) = shared.rest.set_topic(channel, &shared.server_text(topic, &players))
            {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't set the chat channel topic: {e}");
            }
            next_topic = now + topic_every;
        }

        for (u, next) in c.channel_updater.iter().zip(next_names.iter_mut()) {
            if stopping || now < *next || u.channel_id.trim().is_empty() || u.format.is_empty() {
                continue;
            }
            if let Err(e) = shared.rest.set_name(&u.channel_id, &shared.server_text(&u.format, &players)) {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't rename a channel: {e}");
            }
            *next = now + Duration::from_secs(u.update_interval.max(10) * 60);
        }

        if c.server_watchdog_enabled && started && !stopping {
            let since = (shared.epoch.elapsed().as_millis() as u64).saturating_sub(shared.last_tick_ms.load(Ordering::Relaxed));
            let timeout = c.server_watchdog_timeout.max(10) * 1000;
            if since > timeout && !watchdog_fired {
                watchdog_fired = true;
                watchdog(shared, timeout / 1000);
            } else if since <= timeout {
                watchdog_fired = false;
            }
        }

        if stopping {
            shared.drain_jobs(&mut expiring);
            let text = c.discord_chat_channel_server_shutdown_message.trim();
            if let Some(channel) = c.channel_for("status").filter(|_| !text.is_empty()) {
                shared.post_logged(channel, &no_mentions(text), "shutdown message");
            }
            if shared.config.channel_topic_updater_channel_topics_at_shutdown_enabled {
                let shutdown = &c.channel_topic_updater_chat_channel_topic_at_server_shutdown_format;
                if let Some(channel) = main.as_deref().filter(|_| !shutdown.is_empty()) {
                    let _ = shared.rest.set_topic(channel, &shared.server_text(shutdown, &[]));
                }
                for u in c.channel_updater.iter().filter(|u| !u.channel_id.trim().is_empty() && !u.shutdown_format.is_empty()) {
                    let _ = shared.rest.set_name(&u.channel_id, &shared.server_text(&u.shutdown_format, &[]));
                }
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The server hasn't ticked for `timeout` seconds: `ServerWatchdogMessage`, as often as
/// `ServerWatchdogMessageCount` says, pinging who it names.
fn watchdog(shared: &Shared, timeout: u64) {
    let c = &shared.config.chat;
    let Some(channel) = c.channel_for("watchdog") else { return };
    let owner = {
        let st = shared.lock_state();
        st.cache.guild_of_channel(channel).map(|g| format!("<@{}>", g.owner_id)).unwrap_or_default()
    };
    let text = fill(
        &c.server_watchdog_message,
        &[
            ("guildowner", &owner),
            ("date", &shared.date_now()),
            ("timeout", &timeout.to_string()),
            ("timestamp", &Utc::now().timestamp().to_string()),
        ],
    );
    let body = json!({ "content": text, "allowed_mentions": { "parse": ["users", "roles"] } });
    for _ in 0..c.server_watchdog_message_count.max(1) {
        shared.post_logged(channel, &body, "watchdog message");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DEFAULT_CONFIG};

    fn bridge(name: &str) -> (Shared, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hdsrv-relay-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config::parse(DEFAULT_CONFIG).unwrap();
        config.discord_console_channel_id = "500".into();
        config.chat.channels = vec![("global".into(), "300".into())];
        let sh = Shared::for_test(config, &dir);
        sh.dispatch("READY", &json!({ "user": { "id": "1", "username": "Bot" } }));
        sh.dispatch("GUILD_CREATE", &json!({ "id": "100", "owner_id": "7", "roles": [], "channels": [ { "id": "300", "name": "chat" } ] }));
        (sh, dir)
    }

    fn msg(channel: &str, author: &str, content: &str, guild: bool) -> Value {
        let mut d = json!({ "id": "77", "channel_id": channel, "content": content, "author": { "id": author, "username": "user" } });
        if guild {
            d["guild_id"] = json!("100");
            d["member"] = json!({ "roles": [] });
        }
        d
    }

    fn jobs(sh: &Shared) -> Vec<Job> {
        sh.jobs.lock().unwrap().drain(..).collect()
    }

    #[test]
    fn chat_messages_reach_the_game_but_not_the_bots_own() {
        let (sh, dir) = bridge("a");
        sh.dispatch("MESSAGE_CREATE", &msg("300", "9", "hello", true));
        sh.dispatch("MESSAGE_CREATE", &msg("300", "1", "my own echo", true));
        sh.dispatch("MESSAGE_CREATE", &msg("301", "9", "another channel", true));
        let work = std::mem::take(&mut *sh.to_game.lock().unwrap());
        assert_eq!(work.broadcasts, vec!["[§bDiscord§r] user » hello".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_console_channel_still_takes_commands_only() {
        let (mut sh, dir) = bridge("b");
        sh.config.discord_console_channel_blacklist_acts_as_whitelist = false;
        sh.dispatch("MESSAGE_CREATE", &msg("500", "9", "say hi", true));
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["say hi".to_string()]);
        assert!(sh.to_game.lock().unwrap().broadcasts.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Everyone's permissions: nodes that default to everyone.
    fn player(_node: &str, default: u8) -> bool {
        default == 0
    }

    /// An operator: every node.
    fn op(_node: &str, _default: u8) -> bool {
        true
    }

    fn sorted_lines(sh: &Shared, who: u128, args: &str, has: &dyn Fn(&str, u8) -> bool) -> Vec<String> {
        sh.discord_command(who, "a", args, has)
    }

    #[test]
    fn the_player_list_answers_and_expires() {
        let (sh, dir) = bridge("c");
        sh.lock_state().players = vec![
            Player { uuid: 6, name: "zed".into(), dimension: "minecraft:overworld".into() },
            Player { uuid: 5, name: "jeb_".into(), dimension: "minecraft:overworld".into() },
        ];
        sh.dispatch("MESSAGE_CREATE", &msg("300", "9", "PlayerList", true));
        let j = jobs(&sh);
        let Job::Post { channel, body, expire } = &j[0] else { panic!("{j:?}") };
        assert_eq!(channel, "300");
        assert_eq!(body["content"], "**Online players (2/10):**\n```\njeb_, zed\n```");
        assert_eq!(expire, &Some((Duration::from_secs(10), Some("77".into()))));
        assert!(sh.to_game.lock().unwrap().broadcasts.is_empty());
        sh.lock_state().players.clear();
        sh.dispatch("MESSAGE_CREATE", &msg("300", "9", "playerlist", true));
        assert!(matches!(&jobs(&sh)[0], Job::Post { body, .. } if body["content"] == "**No online players**"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn canned_responses_start_a_message_answer_anywhere_and_end_it() {
        let (sh, dir) = bridge("d");
        sh.dispatch("MESSAGE_CREATE", &msg("300", "9", "!IP please", true));
        let j = jobs(&sh);
        assert!(matches!(&j[0], Job::Post { channel, body, .. } if channel == "300" && body["content"] == "yourserveripchange.me"));
        assert!(sh.to_game.lock().unwrap().broadcasts.is_empty());
        // A channel that is no chat channel gets the answer too.
        sh.dispatch("MESSAGE_CREATE", &msg("999", "9", "!site", true));
        assert!(matches!(&jobs(&sh)[0], Job::Post { channel, .. } if channel == "999"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn console_commands_from_a_chat_channel_need_a_role_and_the_list() {
        let (sh, dir) = bridge("k");
        sh.dispatch(
            "GUILD_CREATE",
            &json!({ "id": "100", "owner_id": "7", "roles": [ { "id": "201", "name": "Owner", "color": 0, "position": 5 } ],
                     "channels": [ { "id": "300", "name": "chat" } ] }),
        );
        let mut owner = msg("300", "9", "!c say hi", true);
        owner["member"] = json!({ "roles": ["201"] });
        sh.dispatch("MESSAGE_CREATE", &owner);
        assert_eq!(sh.console_queue.lock().unwrap().drain(..).collect::<Vec<_>>(), vec!["say hi".to_string()]);
        assert_eq!(sh.chat_console.lock().unwrap().iter().cloned().collect::<Vec<_>>(), vec![("say hi".to_string(), "300".to_string())]);
        // The Owner role bypasses the list; nobody else may run anything.
        let mut other = msg("300", "10", "!c say hi", true);
        other["author"]["username"] = json!("pleb");
        sh.dispatch("MESSAGE_CREATE", &other);
        assert!(sh.console_queue.lock().unwrap().is_empty());
        let j = jobs(&sh);
        assert!(matches!(&j[0], Job::Dm { user, content } if user == "10" && content.contains("**pleb**") && content.ends_with("error: no permission")));
        assert!(sh.to_game.lock().unwrap().broadcasts.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn linking_by_pm_from_code_to_role_and_tell() {
        let (sh, dir) = bridge("e");
        let lines = sorted_lines(&sh, 0xAB, "link", &player);
        let code: String = lines[0].chars().filter(char::is_ascii_digit).collect();
        assert_eq!(code.len(), 4, "{lines:?}");
        assert!(lines[0].contains("(Bot)"));
        sh.dispatch("MESSAGE_CREATE", &msg("900", "42", "abc", false));
        sh.dispatch("MESSAGE_CREATE", &msg("900", "42", &format!("my code is {code}!"), false));
        let j = jobs(&sh);
        assert!(matches!(&j[0], Job::Post { body, .. } if body["content"].as_str().unwrap().contains("4 numbers")));
        assert!(matches!(&j[1], Job::Linked { discord, uuid: 0xAB, .. } if discord == "42"));
        assert!(matches!(&j[2], Job::Post { channel, body, .. } if channel == "900"
            && body["content"].as_str().unwrap().starts_with("Your Discord account has been linked to a (")
            && body["message_reference"]["message_id"] == "77"));
        let tells = std::mem::take(&mut sh.to_game.lock().unwrap().tells);
        assert_eq!(tells, vec![(0xAB, "§bYour UUID has been linked to Discord user user (42)".to_string())]);
        assert_eq!(sorted_lines(&sh, 0xAB, "linked", &player), vec!["§bYour Minecraft account is associated with user.".to_string()]);
        assert_eq!(sorted_lines(&sh, 0xAB, "unlink", &op), vec!["§bYour Minecraft account is no longer associated with user.".to_string()]);
        assert!(matches!(jobs(&sh)[0], Job::Unlinked { uuid: 0xAB, .. }));
        assert_eq!(sorted_lines(&sh, 0xAB, "linked", &player), vec!["§cYour Minecraft account isn't associated with a Discord account.".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_link_channel_takes_codes_and_deletes_them_later() {
        let (mut sh, dir) = bridge("l");
        sh.config.chat.channels.push(("link".into(), "301".into()));
        sh.config.chat.minecraft_discord_account_linked_use_pm = false;
        sh.config.chat.minecraft_discord_account_linked_message_delete_seconds = 7;
        sh.dispatch("MESSAGE_CREATE", &msg("301", "42", "1234", true));
        let j = jobs(&sh);
        let Job::Post { channel, expire, .. } = &j[0] else { panic!("{j:?}") };
        assert_eq!(channel, "301");
        assert_eq!(expire, &Some((Duration::from_secs(7), Some("77".into()))));
        assert!(sh.to_game.lock().unwrap().broadcasts.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_discord_command_shows_the_invite() {
        let (sh, dir) = bridge("f");
        let lines = sorted_lines(&sh, 1, "", &player);
        assert_eq!(lines, vec!["§bJoin us on Discord at https://discord.gg/changethisintheconfig.yml§b. For command help, do \"/discord ?\"".to_string()]);
        assert_eq!(sorted_lines(&sh, 1, "bogus", &player), vec!["§bThat command doesn't exist!".to_string()]);
        // Help lists what the player may run, and `help <command>` one entry (colours are random).
        let strip = |v: Vec<String>| v.iter().map(|l| crate::format::strip_codes(l)).collect::<Vec<_>>();
        let help = strip(sorted_lines(&sh, 1, "?", &player));
        assert_eq!(help[0], "================[ DiscordSRV ]================");
        assert!(help.contains(&"- /discord ?/help".to_string()));
        assert!(help.contains(&"    ex. /discord help [command]".to_string()));
        assert!(!help.iter().any(|l| l.contains("/discord reload")));
        assert_eq!(help, strip(sorted_lines(&sh, 1, "HELP", &player)));
        let one = strip(sorted_lines(&sh, 1, "help link", &player));
        assert_eq!(one[0], "===================[ DiscordSRV ]===================");
        assert_eq!(one[1], "- /discord link");
        assert_eq!(one.len(), 3);
        assert_eq!(sorted_lines(&sh, 1, "help nothing", &player), vec!["§bThat command doesn't exist!".to_string()]);
        let all = strip(sorted_lines(&sh, 1, "help", &op));
        assert!(all.contains(&"- /discord broadcast/bcast".to_string()));
        assert!(all.contains(&"- /discord unlink/clearlinked".to_string()));
        assert_eq!(sorted_lines(&sh, 1, "clearlinked", &op), sorted_lines(&sh, 1, "unlink", &op));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staff_subcommands_and_own_unlink_need_the_permission() {
        let (sh, dir) = bridge("h");
        let no = "§cYou do not have permission to perform this command.".to_string();
        for args in ["unlink", "bcast #300 hi", "link 00000000-0000-0000-0000-0000000000ab 42", "reload", "resync", "language de", "debugger"] {
            assert_eq!(sorted_lines(&sh, 1, args, &player), vec![no.clone()], "{args}");
        }
        // The nodes of `others` are asked separately: a player with `link` only.
        let some = |node: &str, d: u8| d == 0 || node == "discordsrv.link";
        assert_eq!(sorted_lines(&sh, 1, "linked someone", &some), vec![no.clone()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_operator_links_broadcasts_and_unlinks_others() {
        let (sh, dir) = bridge("i");
        sh.lock_state().players = vec![Player { uuid: 0xAB, name: "Alex".into(), dimension: String::new() }];
        sh.dispatch("MESSAGE_CREATE", &msg("300", "420000000000000042", "hi", true));
        let lines = sorted_lines(&sh, 1, "link alex 420000000000000042", &op);
        assert_eq!(lines, vec!["§aLinked together §6Alex§a and §6user".to_string()]);
        let j = jobs(&sh);
        assert!(matches!(&j[0], Job::Linked { discord, uuid: 0xAB, .. } if discord == "420000000000000042"));
        // Nobody is told by the command itself.
        assert!(sh.to_game.lock().unwrap().tells.is_empty());
        let linked = sorted_lines(&sh, 1, "linked Alex", &op);
        assert_eq!(linked[0], "§bInterpreted target as §fMinecraft player");
        assert_eq!(linked[1], "§f-§b Player: §fAlex (00000000-0000-0000-0000-0000000000ab)");
        assert_eq!(linked[2], "§f-§b Discord: §fuser (#420000000000000042) (user#0000) 420000000000000042");
        // A broadcast says nothing back.
        assert!(sorted_lines(&sh, 1, "bcast #300 hello\\nworld", &op).is_empty());
        let j = jobs(&sh);
        assert!(matches!(&j[0], Job::Post { channel, body, .. } if channel == "300" && body["content"] == "hello\nworld"));
        assert_eq!(sorted_lines(&sh, 1, "bcast #300", &op), vec!["§cNo text given to broadcast".to_string()]);
        let unlinked = sorted_lines(&sh, 1, "unlink 00000000-0000-0000-0000-0000000000ab", &op);
        assert_eq!(unlinked[0], "§bInterpreted target as §fUUID");
        assert_eq!(unlinked.last().unwrap(), "§f- §b Unlinked ✓");
        assert!(matches!(jobs(&sh)[0], Job::Unlinked { uuid: 0xAB, .. }));
        let nobody = sorted_lines(&sh, 1, "unlink nobody", &op);
        assert!(nobody[0].contains("matching \"nobody\""), "{nobody:?}");
        assert_eq!(sorted_lines(&sh, 1, "link nobody 1", &op), vec!["§cMinecraft player could not be found".to_string()]);
        assert_eq!(sorted_lines(&sh, 1, "resync", &op)[0], "§cGroup synchonization requires valid GroupRoleSynchronizationGroupsAndRolesToSync entries in synchronization.yml");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn language_asks_before_it_changes_anything() {
        let (sh, dir) = bridge("m");
        assert!(sorted_lines(&sh, 1, "language", &op)[0].contains("currently in English"));
        assert!(sorted_lines(&sh, 1, "lang japanese", &op)[0].contains("does not have a translation for Japanese"));
        assert!(sorted_lines(&sh, 1, "setlang en", &op)[0].contains("-confirm"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_placeholders_fill() {
        let (sh, dir) = bridge("g");
        let players = vec![Player { uuid: 5, name: "a".into(), dimension: String::new() }];
        let t = sh.server_text("%playercount%/%playermax% | %totalplayers% | %uptimemins% | %tps%", &players);
        assert_eq!(t, "1/10 | 0 | 0 | 20.0");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
