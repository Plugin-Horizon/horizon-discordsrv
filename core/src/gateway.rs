//! Discord's gateway (v10, JSON, no compression): Hello, Identify with the bot's presence,
//! heartbeats, Resume after a drop or a Reconnect, and every dispatch handed to the bridge
//! (messages of the console and chat channels and PMs, guilds, roles, channels).
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;

use crate::Shared;
use crate::ws::{self, Connection};

/// GUILDS, GUILD_MESSAGES, DIRECT_MESSAGES (link codes) and MESSAGE_CONTENT (privileged: turn
/// it on for the bot). Not the privileged GUILD_MEMBERS: members are learned from messages.
pub const INTENTS: u64 = 1 | (1 << 9) | (1 << 12) | (1 << 15);

/// A message posted in the console channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMessage {
    pub author: String,
    /// The author's Discord user ID (a name can be changed by anyone, the ID not).
    pub author_id: String,
    pub bot: bool,
    pub content: String,
}

/// What ends a connection.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The server stops.
    Stop,
    /// Connect again and resume the session.
    Resume,
    /// Connect again with a new session.
    Identify,
    /// Discord refused the bot for good (bad token, intents...): give up.
    Fatal(String),
}

/// What a close code from Discord means (gateway docs, "Close Event Codes").
pub fn close_outcome(code: u16) -> Outcome {
    match code {
        4004 => Outcome::Fatal("authentication failed: check the bot token".into()),
        4010..=4012 => Outcome::Fatal(format!("gateway close code {code}")),
        4013 | 4014 => Outcome::Fatal("intents refused: turn on the Message Content intent for the bot".into()),
        4007 | 4009 => Outcome::Identify,
        _ => Outcome::Resume,
    }
}

/// The entry of `DiscordGameStatus` shown at `index` (the list cycles; blank entries are skipped).
pub fn status_entry(list: &[String], index: usize) -> Option<&str> {
    let entries: Vec<&str> = list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if entries.is_empty() { None } else { Some(entries[index % entries.len()]) }
}

/// The presence (`d` of op 2 and op 3) for one `DiscordGameStatus` entry and `DiscordOnlineStatus`.
pub fn presence(game_status: Option<&str>, online_status: &str) -> Value {
    let status = match online_status.to_ascii_uppercase().as_str() {
        "IDLE" => "idle",
        "DND" | "DO_NOT_DISTURB" => "dnd",
        "INVISIBLE" | "OFFLINE" => "invisible",
        _ => "online",
    };
    let activities: Vec<Value> = game_status
        .map(|s| {
            let lower = s.to_ascii_lowercase();
            let (kind, name) = [("playing ", 0), ("watching ", 3), ("listening to ", 2), ("competing in ", 5)]
                .iter()
                .find(|(p, _)| lower.starts_with(p))
                .map(|(p, k)| (*k, &s[p.len()..]))
                .unwrap_or((0, s));
            json!({ "name": name, "type": kind })
        })
        .into_iter()
        .collect();
    json!({ "since": null, "activities": activities, "status": status, "afk": false })
}

/// The console channel's message in a MESSAGE_CREATE dispatch, if it is one.
pub fn console_message(d: &Value, channel: &str) -> Option<ChannelMessage> {
    if d.get("channel_id")?.as_str()? != channel {
        return None;
    }
    let author = d.get("author")?;
    Some(ChannelMessage {
        author: author.get("username").and_then(Value::as_str).unwrap_or("?").to_string(),
        author_id: author.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        bot: author.get("bot").and_then(Value::as_bool).unwrap_or(false) || d.get("webhook_id").is_some(),
        content: d.get("content").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

#[derive(Default)]
struct Session {
    id: Option<String>,
    resume_url: Option<String>,
    seq: Option<u64>,
}

/// Runs the gateway until the server stops or Discord refuses the bot for good.
pub fn run(shared: Arc<Shared>, on_dispatch: impl Fn(&str, &Value)) {
    let mut session = Session::default();
    let mut gateway_url: Option<String> = None;
    let mut failures = 0u32;
    while !shared.stop.load(Ordering::Relaxed) {
        let url = match (&session.resume_url, &gateway_url) {
            (Some(u), _) if session.id.is_some() => u.clone(),
            (_, Some(u)) => u.clone(),
            _ => match shared.rest.gateway_url() {
                Ok(u) => {
                    gateway_url = Some(u.clone());
                    u
                }
                Err(e) => {
                    log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Couldn't get the gateway URL: {e}");
                    backoff(&shared, &mut failures);
                    continue;
                }
            },
        };
        let url = format!("{}/?v=10&encoding=json", url.trim_end_matches('/'));
        let outcome = match ws::connect(&url) {
            Ok(conn) => connection(&shared, conn, &mut session, &on_dispatch),
            Err(e) => {
                log::warn!(target: "horizon_discordsrv", "[DiscordSRV] Gateway connection failed: {e}");
                Outcome::Resume
            }
        };
        match outcome {
            Outcome::Stop => break,
            Outcome::Fatal(why) => {
                log::error!(target: "horizon_discordsrv", "[DiscordSRV] Gateway gave up: {why}");
                break;
            }
            Outcome::Identify => {
                session = Session::default();
                backoff(&shared, &mut failures);
            }
            Outcome::Resume => backoff(&shared, &mut failures),
        }
        if shared.ready.load(Ordering::Relaxed) {
            failures = 0;
            shared.ready.store(false, Ordering::Relaxed);
        }
    }
}

/// 1, 2, 4... up to 60 seconds between attempts, cut short when the server stops.
fn backoff(shared: &Shared, failures: &mut u32) {
    let wait = Duration::from_secs((1u64 << (*failures).min(6)).min(60));
    *failures += 1;
    let end = Instant::now() + wait;
    while Instant::now() < end && !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn send(conn: &mut Connection, payload: Value) -> Result<(), String> {
    conn.socket.send(Message::text(payload.to_string())).map_err(|e| e.to_string())
}

fn connection(
    shared: &Shared,
    mut conn: Connection,
    session: &mut Session,
    on_dispatch: &impl Fn(&str, &Value),
) -> Outcome {
    // Hello comes first; then the reads wait a quarter second so heartbeats and the stop flag
    // are looked at in between.
    let interval = match conn.socket.read() {
        Ok(Message::Text(t)) => serde_json::from_str::<Value>(&t)
            .ok()
            .filter(|v| v["op"] == 10)
            .and_then(|v| v["d"]["heartbeat_interval"].as_u64()),
        _ => None,
    };
    let Some(interval) = interval.map(Duration::from_millis) else {
        return Outcome::Resume;
    };
    let _ = conn.tcp.set_read_timeout(Some(Duration::from_millis(250)));
    let hello = if let (Some(id), Some(seq)) = (&session.id, session.seq) {
        json!({ "op": 6, "d": { "token": shared.token, "session_id": id, "seq": seq } })
    } else {
        json!({ "op": 2, "d": {
            "token": shared.token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "horizon-discordsrv", "device": "horizon-discordsrv" },
            "presence": presence(
                status_entry(&shared.config.discord_game_status, shared.status_index.load(Ordering::Relaxed)),
                &shared.config.discord_online_status
            ),
        }})
    };
    if send(&mut conn, hello).is_err() {
        return Outcome::Resume;
    }
    // The first heartbeat after interval * jitter, as the docs ask.
    let jitter = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos()
        % 1000) as f64
        / 1000.0;
    let mut next_beat = Instant::now() + interval.mul_f64(jitter);
    let mut acked = true;
    let status_every = Duration::from_secs(shared.config.status_update_rate_in_minutes.saturating_mul(60));
    let mut next_status = Instant::now() + status_every;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            let _ = conn.socket.close(Some(CloseFrame { code: CloseCode::Normal, reason: "server stopping".into() }));
            let _ = conn.socket.flush();
            return Outcome::Stop;
        }
        if Instant::now() >= next_beat {
            if !acked {
                // No ACK since the last beat: a zombied connection.
                let _ = conn.socket.close(Some(CloseFrame { code: CloseCode::Library(4000), reason: "no heartbeat ack".into() }));
                return Outcome::Resume;
            }
            if send(&mut conn, json!({ "op": 1, "d": session.seq })).is_err() {
                return Outcome::Resume;
            }
            acked = false;
            next_beat = Instant::now() + interval;
        }
        // StatusUpdateRateInMinutes: the presence moves on to the next DiscordGameStatus entry.
        if status_every > Duration::ZERO && Instant::now() >= next_status {
            let index = shared.status_index.fetch_add(1, Ordering::Relaxed) + 1;
            let entry = status_entry(&shared.config.discord_game_status, index);
            let body = json!({ "op": 3, "d": presence(entry, &shared.config.discord_online_status) });
            if send(&mut conn, body).is_err() {
                return Outcome::Resume;
            }
            next_status = Instant::now() + status_every;
        }
        let text = match conn.socket.read() {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(frame)) => {
                return frame.map(|f| close_outcome(u16::from(f.code))).unwrap_or(Outcome::Resume);
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue;
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Outcome::Resume,
            Err(_) => return Outcome::Resume,
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match v["op"].as_u64() {
            Some(0) => {
                if let Some(s) = v["s"].as_u64() {
                    session.seq = Some(s);
                }
                match v["t"].as_str() {
                    Some("READY") => {
                        session.id = v["d"]["session_id"].as_str().map(str::to_string);
                        session.resume_url = v["d"]["resume_gateway_url"].as_str().map(str::to_string);
                        let name = v["d"]["user"]["username"].as_str().unwrap_or("?");
                        log::info!(target: "horizon_discordsrv", "[DiscordSRV] Connected to the gateway as {name}");
                        shared.ready.store(true, Ordering::Relaxed);
                    }
                    Some("RESUMED") => {
                        log::info!(target: "horizon_discordsrv", "[DiscordSRV] Gateway session resumed");
                        shared.ready.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
                if let Some(t) = v["t"].as_str() {
                    on_dispatch(t, &v["d"]);
                }
            }
            Some(1) => {
                if send(&mut conn, json!({ "op": 1, "d": session.seq })).is_err() {
                    return Outcome::Resume;
                }
            }
            Some(7) => {
                let _ = conn.socket.close(Some(CloseFrame { code: CloseCode::Library(4000), reason: "reconnect".into() }));
                return Outcome::Resume;
            }
            Some(9) => {
                return if v["d"].as_bool() == Some(true) { Outcome::Resume } else { Outcome::Identify };
            }
            Some(11) => acked = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_status_is_playing_minecraft_online() {
        let p = presence(Some("playing Minecraft"), "ONLINE");
        assert_eq!(p["status"], "online");
        assert_eq!(p["activities"][0]["name"], "Minecraft");
        assert_eq!(p["activities"][0]["type"], 0);
        let w = presence(Some("Watching the void"), "dnd");
        assert_eq!(w["activities"][0], json!({ "name": "the void", "type": 3 }));
        assert_eq!(w["status"], "dnd");
        assert_eq!(presence(None, "ONLINE")["activities"], json!([]));
    }

    #[test]
    fn the_status_cycles_through_the_entries_and_skips_blank_ones() {
        let list: Vec<String> = vec!["playing Minecraft".into(), "  ".into(), "watching the lobby".into()];
        assert_eq!(status_entry(&list, 0), Some("playing Minecraft"));
        assert_eq!(status_entry(&list, 1), Some("watching the lobby"));
        assert_eq!(status_entry(&list, 2), Some("playing Minecraft"));
        assert_eq!(status_entry(&list, 3), Some("watching the lobby"));
        assert_eq!(status_entry(&[], 5), None);
        assert_eq!(presence(status_entry(&list, 1), "ONLINE")["activities"][0]["type"], 3);
    }

    #[test]
    fn close_codes_decide_what_comes_next() {
        assert!(matches!(close_outcome(4004), Outcome::Fatal(_)));
        assert!(matches!(close_outcome(4014), Outcome::Fatal(_)));
        assert_eq!(close_outcome(4009), Outcome::Identify);
        assert_eq!(close_outcome(4000), Outcome::Resume);
        assert_eq!(close_outcome(1006), Outcome::Resume);
    }

    #[test]
    fn only_the_console_channels_messages_count() {
        let d = json!({ "channel_id": "42", "content": "say hi", "author": { "id": "11", "username": "tester", "bot": false } });
        assert_eq!(
            console_message(&d, "42"),
            Some(ChannelMessage { author: "tester".into(), author_id: "11".into(), bot: false, content: "say hi".into() })
        );
        assert_eq!(console_message(&d, "43"), None);
        let hook = json!({ "channel_id": "42", "content": "x", "webhook_id": "1", "author": { "username": "hook" } });
        assert!(console_message(&hook, "42").unwrap().bot);
    }

    #[test]
    fn intents_are_guilds_messages_pms_and_content() {
        assert_eq!(INTENTS, 37377);
    }
}
