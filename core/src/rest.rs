//! Discord's REST API (v10) through ureq: send a message, set a channel's topic, find the gateway.
use std::time::Duration;

use serde_json::{Value, json};

const USER_AGENT: &str = concat!(
    "DiscordBot (https://github.com/Detimes/horizon-discordsrv, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

pub struct Rest {
    agent: ureq::Agent,
    base: String,
    auth: String,
}

impl Rest {
    pub fn new(base: &str, token: &str) -> Rest {
        Rest {
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build(),
            base: base.trim_end_matches('/').to_string(),
            auth: format!("Bot {token}"),
        }
    }

    /// The gateway's websocket URL (`GET /gateway/bot`).
    pub fn gateway_url(&self) -> Result<String, String> {
        let body = self.call("GET", "/gateway/bot", None)?;
        body.get("url").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "no url in /gateway/bot".into())
    }

    /// Posts `content` to a channel. Mentions in it never ping anyone.
    pub fn send_message(&self, channel: &str, content: &str) -> Result<(), String> {
        let body = json!({ "content": content, "allowed_mentions": { "parse": [] } });
        self.call("POST", &format!("/channels/{channel}/messages"), Some(body)).map(drop)
    }

    pub fn set_topic(&self, channel: &str, topic: &str) -> Result<(), String> {
        self.call("PATCH", &format!("/channels/{channel}"), Some(json!({ "topic": topic }))).map(drop)
    }

    /// Posts a whole message body (content, embeds, allowed_mentions); the new message's ID.
    pub fn post(&self, channel: &str, body: &Value) -> Result<String, String> {
        let answer = self.call("POST", &format!("/channels/{channel}/messages"), Some(body.clone()))?;
        Ok(answer.get("id").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    pub fn delete_message(&self, channel: &str, message: &str) -> Result<(), String> {
        self.call("DELETE", &format!("/channels/{channel}/messages/{message}"), None).map(drop)
    }

    /// Adds the bot's reaction to a message (`emoji` is the unicode emoji itself).
    pub fn add_reaction(&self, channel: &str, message: &str, emoji: &str) -> Result<(), String> {
        let encoded: String = emoji.bytes().map(|b| format!("%{b:02X}")).collect();
        self.call("PUT", &format!("/channels/{channel}/messages/{message}/reactions/{encoded}/@me"), None).map(drop)
    }

    /// Renames a channel (Discord allows two renames per ten minutes).
    pub fn set_name(&self, channel: &str, name: &str) -> Result<(), String> {
        self.call("PATCH", &format!("/channels/{channel}"), Some(json!({ "name": name }))).map(drop)
    }

    /// The PM channel with a user.
    pub fn dm_channel(&self, user: &str) -> Result<String, String> {
        let answer = self.call("POST", "/users/@me/channels", Some(json!({ "recipient_id": user })))?;
        answer.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "no channel id".into())
    }

    pub fn add_role(&self, guild: &str, user: &str, role: &str) -> Result<(), String> {
        self.call("PUT", &format!("/guilds/{guild}/members/{user}/roles/{role}"), None).map(drop)
    }

    pub fn remove_role(&self, guild: &str, user: &str, role: &str) -> Result<(), String> {
        self.call("DELETE", &format!("/guilds/{guild}/members/{user}/roles/{role}"), None).map(drop)
    }

    /// One request; a 429 waits its `retry_after` and tries again (three times at most), a 5xx
    /// once after a second.
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let mut server_errors = 0;
        for _ in 0..4 {
            let request = self
                .agent
                .request(method, &url)
                .set("Authorization", &self.auth)
                .set("User-Agent", USER_AGENT);
            let result = match &body {
                Some(b) => request.set("Content-Type", "application/json").send_string(&b.to_string()),
                // A PUT without a body still needs `Content-Length: 0`.
                None if method == "PUT" => request.send_string(""),
                None => request.call(),
            };
            match result {
                Ok(response) => {
                    let text = response.into_string().map_err(|e| e.to_string())?;
                    return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
                }
                Err(ureq::Error::Status(429, response)) => {
                    let wait = retry_after(response.into_string().unwrap_or_default().as_str());
                    std::thread::sleep(Duration::from_secs_f64(wait.min(60.0)));
                }
                Err(ureq::Error::Status(code, _)) if code >= 500 && server_errors == 0 => {
                    server_errors += 1;
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(ureq::Error::Status(code, response)) => {
                    let text = response.into_string().unwrap_or_default();
                    return Err(format!("{method} {path}: HTTP {code} {}", text.chars().take(200).collect::<String>()));
                }
                Err(e) => return Err(format!("{method} {path}: {}", transport_error(&e))),
            }
        }
        Err(format!("{method} {path}: still rate limited"))
    }
}

/// The seconds a 429's JSON body asks to wait (1 when it says nothing).
pub fn retry_after(body: &str) -> f64 {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("retry_after").and_then(Value::as_f64))
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(1.0)
}

/// A transport error without the request's URL (it holds no secret, but keep logs short).
fn transport_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Transport(t) => t.kind().to_string(),
        other => other.kind().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_discords_body() {
        assert_eq!(retry_after(r#"{"message":"You are being rate limited.","retry_after":0.25,"global":false}"#), 0.25);
        assert_eq!(retry_after("not json"), 1.0);
        assert_eq!(retry_after(r#"{"retry_after":-3}"#), 1.0);
    }
}
