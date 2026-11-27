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
    /// The permission bit field of the role.
    pub permissions: u64,
}

/// A channel's permission overwrite (`permission_overwrites`): `kind` 0 is a role, 1 a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    pub id: String,
    pub kind: u8,
    pub allow: u64,
    pub deny: u64,
}

/// What the gateway tells of a channel beyond its name: its type (2 voice, 4 category), its category
/// and its overwrites.
#[derive(Debug, Clone, Default)]
pub struct ChannelInfo {
    pub kind: u8,
    pub parent: Option<String>,
    pub overwrites: Vec<Overwrite>,
}

/// Discord permission bits the port checks.
pub mod perm {
    pub const ADMINISTRATOR: u64 = 1 << 3;
    pub const MANAGE_CHANNELS: u64 = 1 << 4;
    pub const VIEW_CHANNEL: u64 = 1 << 10;
    pub const CONNECT: u64 = 1 << 20;
    pub const SPEAK: u64 = 1 << 21;
    pub const MUTE_MEMBERS: u64 = 1 << 22;
    pub const MOVE_MEMBERS: u64 = 1 << 24;
    pub const MANAGE_ROLES: u64 = 1 << 28;
}

#[derive(Debug, Clone, Default)]
pub struct Guild {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub icon: String,
    pub splash: String,
    /// `member_count` of the guild (the placeholder `guild_members_total`).
    pub member_count: u64,
    /// Members online, as the last count the chat thread asked Discord for (`guild_members_online`).
    pub online: Option<u64>,
    pub roles: HashMap<String, Role>,
}

#[derive(Debug, Clone, Default)]
pub struct Member {
    pub username: String,
    pub global_name: Option<String>,
    /// Per guild nickname, as last seen.
    pub nick: Option<String>,
    pub roles: Vec<String>,
    /// The user's avatar (their own, or Discord's default one): `getEffectiveAvatarUrl`.
    pub avatar_url: String,
    /// The member boosts the guild (`premium_since` is set).
    pub boosting: bool,
}

/// What a member is doing (`PRESENCE_UPDATE`, needs the presences intent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Presence {
    /// `online`, `idle`, `dnd` or `offline` (JDA's `OnlineStatus#getKey`).
    pub status: String,
    /// The first activity's name and URL.
    pub game: Option<(String, Option<String>)>,
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
    /// Channel id -> type, category and overwrites.
    pub channel_info: HashMap<String, ChannelInfo>,
    /// User id -> the voice channel they are in (users in no voice channel are absent).
    pub voice_states: HashMap<String, String>,
    /// Users the guild has muted (server mute).
    pub guild_muted: std::collections::HashSet<String>,
    /// The last `VOICE_STATE_UPDATE`: (user, channel left, channel joined), taken by the voice module.
    pub voice_change: Option<(String, Option<String>, Option<String>)>,
    /// User id -> presence (only with the presences intent).
    pub presences: HashMap<String, Presence>,
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
        permissions: v.get("permissions").and_then(|p| p.as_str().and_then(|t| t.parse().ok()).or_else(|| p.as_u64())).unwrap_or(0),
    })
}

fn overwrite(v: &Value) -> Option<Overwrite> {
    let bits = |k: &str| v.get(k).and_then(|p| p.as_str().and_then(|t| t.parse().ok()).or_else(|| p.as_u64())).unwrap_or(0);
    Some(Overwrite {
        id: v.get("id")?.as_str()?.to_string(),
        kind: v.get("type").and_then(|t| t.as_u64().or_else(|| t.as_str().and_then(|x| x.parse().ok()))).unwrap_or(0) as u8,
        allow: bits("allow"),
        deny: bits("deny"),
    })
}

fn channel_info(c: &Value) -> ChannelInfo {
    ChannelInfo {
        kind: c.get("type").and_then(Value::as_u64).unwrap_or(0) as u8,
        parent: c.get("parent_id").and_then(Value::as_str).map(str::to_string),
        overwrites: c.get("permission_overwrites").and_then(Value::as_array).map(|a| a.iter().filter_map(overwrite).collect()).unwrap_or_default(),
    }
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
                guild.name = s(d, "name");
                guild.icon = s(d, "icon");
                guild.splash = s(d, "splash");
                if let Some(n) = d.get("member_count").and_then(Value::as_u64) {
                    guild.member_count = n;
                }
                if let Some(roles) = d.get("roles").and_then(Value::as_array) {
                    guild.roles = roles.iter().filter_map(role).map(|r| (r.id.clone(), r)).collect();
                }
                for c in d.get("channels").and_then(Value::as_array).into_iter().flatten() {
                    self.channels.insert(s(c, "id"), (s(c, "name"), id.clone()));
                    self.channel_info.insert(s(c, "id"), channel_info(c));
                }
                // Threads are channels of the guild too, but nothing here needs them.
                for p in d.get("presences").and_then(Value::as_array).into_iter().flatten() {
                    self.presence(p);
                }
                for v in d.get("voice_states").and_then(Value::as_array).into_iter().flatten() {
                    self.voice_state(v);
                }
                for m in d.get("members").and_then(Value::as_array).into_iter().flatten() {
                    self.member(&m["user"], Some(m));
                }
            }
            "GUILD_DELETE" => {
                let id = s(d, "id");
                self.guilds.remove(&id);
                let gone: Vec<String> = self.channels.iter().filter(|(_, (_, g))| *g == id).map(|(c, _)| c.clone()).collect();
                for c in gone {
                    self.channels.remove(&c);
                    self.channel_info.remove(&c);
                }
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
                    self.channel_info.insert(s(d, "id"), channel_info(d));
                }
            }
            "CHANNEL_DELETE" => {
                self.channels.remove(&s(d, "id"));
                self.channel_info.remove(&s(d, "id"));
            }
            "PRESENCE_UPDATE" => self.presence(d),
            "VOICE_STATE_UPDATE" => {
                if let Some(m) = d.get("member").filter(|m| m.get("user").is_some()) {
                    self.member(&m["user"], Some(m));
                }
                let user = s(d, "user_id");
                let old = self.voice_states.get(&user).cloned();
                self.voice_state(d);
                let new = self.voice_states.get(&user).cloned();
                if !user.is_empty() {
                    self.voice_change = Some((user, old, new));
                }
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

    fn presence(&mut self, d: &Value) {
        let user = d.get("user").map(|u| s(u, "id")).unwrap_or_default();
        if user.is_empty() {
            return;
        }
        let game = d.get("activities").and_then(Value::as_array).and_then(|a| a.first()).map(|a| {
            (s(a, "name"), a.get("url").and_then(Value::as_str).map(str::to_string))
        });
        self.presences.insert(user, Presence { status: s(d, "status"), game });
    }

    fn voice_state(&mut self, d: &Value) {
        let user = s(d, "user_id");
        if user.is_empty() {
            return;
        }
        match d.get("channel_id").and_then(Value::as_str) {
            Some(channel) => {
                self.voice_states.insert(user.clone(), channel.to_string());
            }
            None => {
                self.voice_states.remove(&user);
            }
        }
        if d.get("mute").and_then(Value::as_bool) == Some(true) {
            self.guild_muted.insert(user);
        } else {
            self.guild_muted.remove(&user);
        }
    }

    fn member(&mut self, user: &Value, member: Option<&Value>) {
        let id = s(user, "id");
        if id.is_empty() {
            return;
        }
        let avatar = s(user, "avatar");
        let avatar_url = if avatar.is_empty() {
            // Discord's default avatars: picked by the user ID (the new username system).
            let index = id.parse::<u64>().map(|n| (n >> 22) % 6).unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
        } else {
            format!("https://cdn.discordapp.com/avatars/{id}/{avatar}.png")
        };
        let entry = self.members.entry(id).or_default();
        entry.avatar_url = avatar_url;
        entry.username = s(user, "username");
        entry.global_name = user.get("global_name").and_then(Value::as_str).map(str::to_string);
        if let Some(m) = member {
            entry.boosting = m.get("premium_since").is_some_and(|p| !p.is_null());
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

    /// The channel's category, when it has one.
    pub fn parent_of(&self, channel: &str) -> Option<&str> {
        self.channel_info.get(channel).and_then(|i| i.parent.as_deref())
    }

    /// The permissions `user` (with member roles `roles`) holds in `channel`: Discord's algorithm
    /// (base permissions of @everyone and the roles, then the channel's overwrites for @everyone, the
    /// roles and the member). The owner and administrators hold everything.
    pub fn permissions_in(&self, guild: &Guild, user: &str, roles: &[String], channel: &str) -> u64 {
        if guild.owner_id == user {
            return u64::MAX;
        }
        let mut base = guild.roles.get(&guild.id).map(|r| r.permissions).unwrap_or(0);
        for r in roles {
            base |= guild.roles.get(r).map(|r| r.permissions).unwrap_or(0);
        }
        if base & perm::ADMINISTRATOR != 0 {
            return u64::MAX;
        }
        let Some(info) = self.channel_info.get(channel) else {
            return base;
        };
        let mut p = base;
        if let Some(o) = info.overwrites.iter().find(|o| o.kind == 0 && o.id == guild.id) {
            p = (p & !o.deny) | o.allow;
        }
        let (mut deny, mut allow) = (0, 0);
        for o in info.overwrites.iter().filter(|o| o.kind == 0 && roles.contains(&o.id)) {
            deny |= o.deny;
            allow |= o.allow;
        }
        p = (p & !deny) | allow;
        if let Some(o) = info.overwrites.iter().find(|o| o.kind == 1 && o.id == user) {
            p = (p & !o.deny) | o.allow;
        }
        p
    }

    /// The roles a seen member holds (`None` for one never seen).
    pub fn roles_of(&self, user: &str) -> Option<&[String]> {
        self.members.get(user).map(|m| m.roles.as_slice())
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
