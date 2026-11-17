//! What the gateway tells about the bot's guilds, kept for the chat side: roles (names, colours,
//! order), channel names, the owner, and the members seen in messages (without the privileged
//! members intent there is no full member list).
use std::collections::HashMap;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub id: String,
    pub name: String,
    /// RGB; 0 is "no colour".
    pub color: u32,
    pub position: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Guild {
    pub id: String,
    pub owner_id: String,
    pub roles: HashMap<String, Role>,
}

#[derive(Debug, Clone, Default)]
pub struct Member {
    pub username: String,
    pub global_name: Option<String>,
    /// Per guild nickname, as last seen.
    pub nick: Option<String>,
    pub roles: Vec<String>,
}

impl Member {
    /// Nickname, else username: JDA 4's `Member.getEffectiveName()`, which DiscordSRV uses (it
    /// knows no global display names).
    pub fn effective_name(&self) -> &str {
        self.nick.as_deref().unwrap_or(&self.username)
    }
}

#[derive(Debug, Default)]
pub struct Cache {
    pub bot_id: String,
    pub bot_name: String,
    pub bot_avatar: String,
    pub guilds: HashMap<String, Guild>,
    /// Channel id -> (name, guild id).
    pub channels: HashMap<String, (String, String)>,
    pub members: HashMap<String, Member>,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn role(v: &Value) -> Option<Role> {
    Some(Role {
        id: v.get("id")?.as_str()?.to_string(),
        name: s(v, "name"),
        color: v.get("color").and_then(Value::as_u64).unwrap_or(0) as u32,
        position: v.get("position").and_then(Value::as_i64).unwrap_or(0),
    })
}

impl Cache {
    /// Takes in a dispatch (`t`, `d`).
    pub fn apply(&mut self, event: &str, d: &Value) {
        match event {
            "READY" => {
                let user = &d["user"];
                self.bot_id = s(user, "id");
                self.bot_name = s(user, "username");
                let avatar = s(user, "avatar");
                self.bot_avatar = if avatar.is_empty() {
                    String::new()
                } else {
                    format!("https://cdn.discordapp.com/avatars/{}/{avatar}.png", self.bot_id)
                };
            }
            "GUILD_CREATE" | "GUILD_UPDATE" => {
                let id = s(d, "id");
                let guild = self.guilds.entry(id.clone()).or_default();
                guild.id.clone_from(&id);
                guild.owner_id = s(d, "owner_id");
                if let Some(roles) = d.get("roles").and_then(Value::as_array) {
                    guild.roles = roles.iter().filter_map(role).map(|r| (r.id.clone(), r)).collect();
                }
                for c in d.get("channels").and_then(Value::as_array).into_iter().flatten() {
                    self.channels.insert(s(c, "id"), (s(c, "name"), id.clone()));
                }
                for m in d.get("members").and_then(Value::as_array).into_iter().flatten() {
                    self.member(&m["user"], Some(m));
                }
            }
            "GUILD_DELETE" => {
                let id = s(d, "id");
                self.guilds.remove(&id);
                self.channels.retain(|_, (_, g)| *g != id);
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
                if let (Some(guild), Some(r)) = (self.guilds.get_mut(&s(d, "guild_id")), role(&d["role"])) {
                    guild.roles.insert(r.id.clone(), r);
                }
            }
            "GUILD_ROLE_DELETE" => {
                if let Some(guild) = self.guilds.get_mut(&s(d, "guild_id")) {
                    guild.roles.remove(&s(d, "role_id"));
                }
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
                if d.get("guild_id").is_some() {
                    self.channels.insert(s(d, "id"), (s(d, "name"), s(d, "guild_id")));
                }
            }
            "CHANNEL_DELETE" => {
                self.channels.remove(&s(d, "id"));
            }
            "MESSAGE_CREATE" => {
                if d.get("webhook_id").is_none() {
                    self.member(&d["author"], d.get("member"));
                }
                for m in d.get("mentions").and_then(Value::as_array).into_iter().flatten() {
                    self.member(m, m.get("member"));
                }
            }
            "GUILD_MEMBER_UPDATE" => self.member(&d["user"], Some(d)),
            _ => {}
        }
    }

    fn member(&mut self, user: &Value, member: Option<&Value>) {
        let id = s(user, "id");
        if id.is_empty() {
            return;
        }
        let entry = self.members.entry(id).or_default();
        entry.username = s(user, "username");
        entry.global_name = user.get("global_name").and_then(Value::as_str).map(str::to_string);
        if let Some(m) = member {
            entry.nick = m.get("nick").and_then(Value::as_str).map(str::to_string);
            if let Some(roles) = m.get("roles").and_then(Value::as_array) {
                entry.roles = roles.iter().filter_map(Value::as_str).map(str::to_string).collect();
            }
        }
    }

    pub fn guild_of_channel(&self, channel: &str) -> Option<&Guild> {
        self.channels.get(channel).and_then(|(_, g)| self.guilds.get(g)).or_else(|| self.single_guild())
    }

    /// The only guild, when the bot is in one (the usual case).
    pub fn single_guild(&self) -> Option<&Guild> {
        if self.guilds.len() == 1 { self.guilds.values().next() } else { None }
    }

    pub fn channel_name(&self, id: &str) -> Option<&str> {
        self.channels.get(id).map(|(n, _)| n.as_str())
    }

    /// `ids` as roles of `guild`, highest first.
    pub fn sorted_roles(&self, guild: &Guild, ids: &[String]) -> Vec<Role> {
        let mut roles: Vec<Role> = ids.iter().filter_map(|id| guild.roles.get(id)).cloned().collect();
        roles.sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
        roles
    }

    /// A role by ID or name (case-insensitive).
    pub fn find_role<'a>(&self, guild: &'a Guild, name_or_id: &str) -> Option<&'a Role> {
        guild.roles.get(name_or_id).or_else(|| guild.roles.values().find(|r| r.name.eq_ignore_ascii_case(name_or_id)))
    }

    /// A seen member whose name, display name or nickname is `name` (case-insensitive).
    pub fn find_member(&self, name: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|(_, m)| {
                m.username.eq_ignore_ascii_case(name)
                    || m.nick.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))
                    || m.global_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|(id, _)| id.as_str())
    }

    pub fn find_channel(&self, name: &str) -> Option<&str> {
        self.channels.iter().find(|(_, (n, _))| n.eq_ignore_ascii_case(name)).map(|(id, _)| id.as_str())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;

    pub fn sample() -> Cache {
        let mut c = Cache::default();
        c.apply("READY", &json!({ "user": { "id": "1", "username": "Bot", "avatar": "abc" } }));
        c.apply(
            "GUILD_CREATE",
            &json!({
                "id": "100", "owner_id": "7",
                "roles": [
                    { "id": "100", "name": "@everyone", "color": 0, "position": 0 },
                    { "id": "201", "name": "Owner", "color": 0xE74C3C, "position": 5 },
                    { "id": "202", "name": "Developer", "color": 0x3498DB, "position": 3 },
                    { "id": "203", "name": "Misc role", "color": 0, "position": 1 },
                ],
                "channels": [ { "id": "300", "name": "server-chat" }, { "id": "301", "name": "rules" } ],
            }),
        );
        c
    }

    #[test]
    fn guild_create_fills_roles_and_channels() {
        let c = sample();
        assert_eq!(c.bot_avatar, "https://cdn.discordapp.com/avatars/1/abc.png");
        let g = c.guild_of_channel("300").unwrap();
        assert_eq!(g.owner_id, "7");
        let roles = c.sorted_roles(g, &["203".into(), "202".into(), "201".into()]);
        assert_eq!(roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Owner", "Developer", "Misc role"]);
        assert_eq!(c.find_channel("RULES"), Some("301"));
        assert_eq!(c.find_role(g, "developer").map(|r| r.id.as_str()), Some("202"));
    }

    #[test]
    fn members_are_learned_from_messages() {
        let mut c = sample();
        c.apply(
            "MESSAGE_CREATE",
            &json!({ "author": { "id": "9", "username": "notch", "global_name": "Notch" },
                     "member": { "nick": "NotchIsMe", "roles": ["202"] } }),
        );
        assert_eq!(c.members["9"].effective_name(), "NotchIsMe");
        assert_eq!(c.find_member("notchisme"), Some("9"));
        c.apply("GUILD_ROLE_DELETE", &json!({ "guild_id": "100", "role_id": "202" }));
        assert!(!c.guilds["100"].roles.contains_key("202"));
    }
}
