//! DiscordSRV's PlaceholderAPI expansion (`hooks/PlaceholderAPIExpansion`): the `%discordsrv_...%` values.
//! Horizon's PlaceholderAPI port reaches them through the mod service `discordsrv:placeholder` (see the
//! README), with the player the placeholder is for and the identifier after `discordsrv_`.
//!
//! The values come from the gateway's cache. What needs the privileged presences intent (`user_online_status`,
//! the `*_game_*` values, `linked_online`) answers only when the `EnablePresenceInformation` key is on.
use std::sync::atomic::Ordering;

use crate::discordsrv_bridge::Shared;
use crate::discordsrv_bridge::format::nearest_color;
use crate::discordsrv_bridge::guild::{Cache, Guild, Role};

/// PlaceholderAPI's default `boolean_true` / `boolean_false`.
fn boolean(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

fn hex(color: u32) -> String {
    format!("#{:02x}{:02x}{:02x}", (color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF)
}

/// `Component.text(0).color(...)` as legacy text, without the digit: the section code of the nearest chat colour.
fn color_code(color: u32) -> String {
    format!("\u{a7}{}", nearest_color(color))
}

/// A role's value: 0 is no colour, which JDA reports as `Role.DEFAULT_COLOR_RAW` (0x1FFFFFFF) and Java renders as an empty string.
fn role_value(role: &Role, what: &str) -> Option<String> {
    Some(match what {
        "id" => role.id.clone(),
        "name" => role.name.clone(),
        "color_hex" if role.color != 0 => hex(role.color),
        "color_code" if role.color != 0 => color_code(role.color),
        "color_hex" | "color_code" => String::new(),
        _ => return None,
    })
}

/// The `role_<id>_<what>` form: (id, what).
fn specific_role(identifier: &str) -> Option<(&str, &str)> {
    let rest = identifier.strip_prefix("role_")?;
    let (id, what) = rest.split_once('_')?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !what.is_empty() && what.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then_some((id, what))
}

fn effective_name(cache: &Cache, user: &str) -> String {
    cache.members.get(user).map(|m| m.effective_name().to_string()).unwrap_or_default()
}

fn game(cache: &Cache, user: &str, url: bool) -> String {
    cache
        .presences
        .get(user)
        .and_then(|p| p.game.as_ref())
        .map(|(name, link)| if url { link.clone().unwrap_or_default() } else { name.clone() })
        .unwrap_or_default()
}

/// The guild the bot's main chat channel is in (`DiscordSRV#getMainGuild`), else the only guild.
fn main_guild<'a>(sh: &Shared, cache: &'a Cache) -> Option<&'a Guild> {
    sh.config.chat.channel_for("global").and_then(|c| cache.guild_of_channel(c)).or_else(|| cache.single_guild())
}

/// The value of `identifier` (what follows `discordsrv_`) for the player `uuid` (none: a server-wide
/// placeholder). `None` is "not one of ours": PlaceholderAPI shows the placeholder as it is.
pub fn value(sh: &Shared, uuid: Option<u128>, identifier: &str) -> Option<String> {
    let st = sh.lock_state();
    let cache = &st.cache;
    let Some(guild) = main_guild(sh, cache) else {
        return Some(String::new());
    };
    let presence = sh.config.enable_presence_information;
    let owner = |what: &str| -> String {
        match what {
            "effective_name" => effective_name(cache, &guild.owner_id),
            "nickname" => cache.members.get(&guild.owner_id).and_then(|m| m.nick.clone()).unwrap_or_default(),
            "game_name" => game(cache, &guild.owner_id, false),
            _ => game(cache, &guild.owner_id, true),
        }
    };
    let bot = |what: &str| -> String {
        match what {
            "effective_name" => {
                let m = cache.members.get(&cache.bot_id);
                m.map(|m| m.effective_name().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| cache.bot_name.clone())
            }
            "nickname" => cache.members.get(&cache.bot_id).and_then(|m| m.nick.clone()).unwrap_or_default(),
            "game_name" => game(cache, &cache.bot_id, false),
            _ => game(cache, &cache.bot_id, true),
        }
    };
    let cdn = |kind: &str, hash: &str| {
        if hash.is_empty() {
            String::new()
        } else {
            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/{kind}/{}/{hash}.{ext}", guild.id)
        }
    };
    let linked: Vec<u128> = st.accounts.all_uuids();
    match identifier {
        "guild_id" => return Some(guild.id.clone()),
        "guild_name" => return Some(guild.name.clone()),
        "guild_icon_id" => return Some(guild.icon.clone()),
        "guild_icon_url" => return Some(cdn("icons", &guild.icon)),
        "guild_splash_id" => return Some(guild.splash.clone()),
        "guild_splash_url" => return Some(cdn("splashes", &guild.splash)),
        "guild_owner_effective_name" => return Some(owner("effective_name")),
        "guild_owner_nickname" => return Some(owner("nickname")),
        "guild_owner_game_name" => return Some(owner("game_name")),
        "guild_owner_game_url" => return Some(owner("game_url")),
        "guild_bot_effective_name" => return Some(bot("effective_name")),
        "guild_bot_nickname" => return Some(bot("nickname")),
        "guild_bot_game_name" => return Some(bot("game_name")),
        "guild_bot_game_url" => return Some(bot("game_url")),
        "guild_members_online" => {
            let online = if presence {
                Some(cache.presences.values().filter(|p| p.status != "offline" && !p.status.is_empty()).count() as u64)
            } else {
                // The chat thread asks Discord for the count about once a minute, from the first request on.
                sh.counts_wanted.store(true, Ordering::Relaxed);
                guild.online
            };
            return Some(online.map(|n| n.to_string()).unwrap_or_default());
        }
        "guild_members_total" => return Some(guild.member_count.to_string()),
        "linked_online" => {
            if !presence {
                return Some(String::new());
            }
            let online = linked
                .iter()
                .filter_map(|u| st.accounts.discord_of(*u))
                .filter(|d| cache.presences.get(*d).is_some_and(|p| p.status != "offline" && !p.status.is_empty()))
                .count();
            return Some(online.to_string());
        }
        "linked_total" => return Some(linked.len().to_string()),
        _ => {}
    }
    if let Some((id, what)) = specific_role(identifier) {
        return Some(guild.roles.get(id).and_then(|r| role_value(r, what)).unwrap_or_default());
    }
    let Some(uuid) = uuid else { return Some(String::new()) };
    let user = st.accounts.discord_of(uuid).map(str::to_string);
    match identifier {
        "user_id" => return Some(user.unwrap_or_default()),
        "user_islinked" => return Some(boolean(user.is_some())),
        _ => {}
    }
    let Some(user) = user else { return Some(String::new()) };
    let Some(member) = cache.members.get(&user) else { return Some(String::new()) };
    match identifier {
        "user_name" | "user_tag" => return Some(member.username.clone()),
        "user_effective_name" => return Some(member.effective_name().to_string()),
        "user_nickname" => return Some(member.nick.clone().unwrap_or_default()),
        "user_online_status" => {
            return Some(if presence { cache.presences.get(&user).map(|p| p.status.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "offline".into()) } else { String::new() });
        }
        "user_game_name" => return Some(game(cache, &user, false)),
        "user_game_url" => return Some(game(cache, &user, true)),
        "user_boost_status" => return Some(boolean(member.boosting)),
        _ => {}
    }
    if identifier == "user_roles" {
        let names: Vec<String> = cache.sorted_roles(guild, &member.roles).into_iter().filter(|r| r.id != guild.id && !r.name.trim().is_empty()).map(|r| r.name.clone()).collect();
        return Some(names.join(", "));
    }
    if member.roles.is_empty() {
        return Some(String::new());
    }
    let roles = cache.sorted_roles(guild, &member.roles);
    let roles: Vec<Role> = roles.into_iter().filter(|r| r.id != guild.id).collect();
    let selection = &sh.config.chat.discord_chat_channel_roles_selection;
    let selected = roles
        .iter()
        .find(|r| selection.iter().any(|n| *n == r.name || *n == r.id) == sh.config.chat.discord_chat_channel_roles_selection_as_whitelist && !r.name.trim().is_empty());
    for (prefix, role) in [("user_top_selected_role_", selected), ("user_top_role_", roles.first())] {
        if let Some(what) = identifier.strip_prefix(prefix)
            && let Some(role) = role
        {
            return Some(role_value(role, what).unwrap_or_default());
        }
    }
    None
}

/// The LuckPerms contexts `DiscordSRV` offers (`LuckPermsHook#calculate`): `discordsrv:linked` always, and for a
/// linked member of the main guild `discordsrv:server_id`, `discordsrv:boosting`, and a `discordsrv:role` / `discordsrv:role_id`
/// pair for each role that has a name.
pub fn contexts(sh: &Shared, uuid: u128) -> Vec<(String, String)> {
    let st = sh.lock_state();
    let mut out = Vec::new();
    let user = st.accounts.discord_of(uuid);
    out.push(("discordsrv:linked".to_string(), user.is_some().to_string()));
    let Some(user) = user else { return out };
    let Some(member) = st.cache.members.get(user) else { return out };
    let Some(guild) = main_guild(sh, &st.cache) else { return out };
    out.push(("discordsrv:server_id".to_string(), guild.id.clone()));
    out.push(("discordsrv:boosting".to_string(), member.boosting.to_string()));
    for role in st.cache.sorted_roles(guild, &member.roles) {
        if role.name.trim().is_empty() || role.id == guild.id {
            continue;
        }
        out.push(("discordsrv:role".to_string(), role.name.clone()));
        out.push(("discordsrv:role_id".to_string(), role.id.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discordsrv_bridge::config::Config;
    use serde_json::json;

    fn bridge(name: &str) -> (Shared, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("discordsrv-papi-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut config = Config::default();
        config.chat.channels = vec![("global".into(), "300".into())];
        let sh = Shared::for_test(config, &dir);
        sh.dispatch("READY", &json!({ "user": { "id": "1", "username": "Bot" } }));
        sh.dispatch(
            "GUILD_CREATE",
            &json!({
                "id": "100", "owner_id": "7", "name": "Core", "icon": "a_abc", "member_count": 12,
                "roles": [
                    { "id": "100", "name": "@everyone", "color": 0, "position": 0 },
                    { "id": "201", "name": "Owner", "color": 0xE74C3C, "position": 5 },
                    { "id": "202", "name": "Dev", "color": 0x3498DB, "position": 3 },
                ],
                "channels": [ { "id": "300", "name": "chat" } ],
                "members": [
                    { "user": { "id": "9", "username": "notch" }, "nick": "N", "roles": ["202", "201"], "premium_since": "2026-01-01T00:00:00Z" },
                    { "user": { "id": "7", "username": "boss" }, "roles": [] },
                ],
            }),
        );
        (sh, dir)
    }

    #[test]
    fn guild_values_come_from_the_cache() {
        let (sh, dir) = bridge("a");
        let v = |i: &str| value(&sh, None, i);
        assert_eq!(v("guild_id").as_deref(), Some("100"));
        assert_eq!(v("guild_name").as_deref(), Some("Core"));
        assert_eq!(v("guild_icon_url").as_deref(), Some("https://cdn.discordapp.com/icons/100/a_abc.gif"));
        assert_eq!(v("guild_splash_url").as_deref(), Some(""));
        assert_eq!(v("guild_members_total").as_deref(), Some("12"));
        assert_eq!(v("guild_owner_effective_name").as_deref(), Some("boss"));
        assert_eq!(v("guild_bot_effective_name").as_deref(), Some("Bot"));
        assert_eq!(v("linked_total").as_deref(), Some("0"));
        // Presence values need the intent: empty without it.
        assert_eq!(v("linked_online").as_deref(), Some(""));
        // Without a player an unknown identifier is empty (PlaceholderAPI shows nothing).
        assert_eq!(v("no_such_thing").as_deref(), Some(""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn role_values_use_the_roles_id() {
        let (sh, dir) = bridge("b");
        assert_eq!(value(&sh, None, "role_201_name").as_deref(), Some("Owner"));
        assert_eq!(value(&sh, None, "role_201_color_hex").as_deref(), Some("#e74c3c"));
        assert_eq!(value(&sh, None, "role_202_color_code"), Some(format!("\u{a7}{}", nearest_color(0x3498DB))));
        assert_eq!(value(&sh, None, "role_999_name").as_deref(), Some(""));
        assert_eq!(specific_role("role_12_color_hex"), Some(("12", "color_hex")));
        assert_eq!(specific_role("role_x_name"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn member_values_follow_the_link() {
        let (sh, dir) = bridge("c");
        let me = 0xABu128;
        assert_eq!(value(&sh, Some(me), "user_islinked").as_deref(), Some("no"));
        assert_eq!(value(&sh, Some(me), "user_id").as_deref(), Some(""));
        sh.lock_state().accounts.link(me, "9").unwrap();
        assert_eq!(value(&sh, Some(me), "user_islinked").as_deref(), Some("yes"));
        assert_eq!(value(&sh, Some(me), "user_id").as_deref(), Some("9"));
        assert_eq!(value(&sh, Some(me), "user_name").as_deref(), Some("notch"));
        assert_eq!(value(&sh, Some(me), "user_effective_name").as_deref(), Some("N"));
        assert_eq!(value(&sh, Some(me), "user_nickname").as_deref(), Some("N"));
        assert_eq!(value(&sh, Some(me), "user_boost_status").as_deref(), Some("yes"));
        assert_eq!(value(&sh, Some(me), "user_top_role_name").as_deref(), Some("Owner"));
        assert_eq!(value(&sh, Some(me), "user_top_role_id").as_deref(), Some("201"));
        assert!(value(&sh, Some(me), "user_roles").unwrap().split(", ").any(|r| r == "Owner"));
        assert_eq!(value(&sh, Some(me), "user_online_status").as_deref(), Some(""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn luckperms_contexts_follow_the_link() {
        let (sh, dir) = bridge("d");
        assert_eq!(contexts(&sh, 5), vec![("discordsrv:linked".to_string(), "false".to_string())]);
        sh.lock_state().accounts.link(5, "9").unwrap();
        let c = contexts(&sh, 5);
        assert_eq!(c[0], ("discordsrv:linked".to_string(), "true".to_string()));
        assert!(c.contains(&("discordsrv:server_id".to_string(), "100".to_string())));
        assert!(c.contains(&("discordsrv:boosting".to_string(), "true".to_string())));
        assert!(c.contains(&("discordsrv:role".to_string(), "Owner".to_string())));
        assert!(c.contains(&("discordsrv:role_id".to_string(), "202".to_string())));
        assert!(!c.iter().any(|(k, v)| k == "discordsrv:role" && v == "@everyone"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn presences_answer_when_the_intent_is_on() {
        let dir = std::env::temp_dir().join(format!("discordsrv-papi2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut config = Config::default();
        config.enable_presence_information = true;
        config.chat.channels = vec![("global".into(), "300".into())];
        let sh = Shared::for_test(config, &dir);
        sh.dispatch("GUILD_CREATE", &json!({ "id": "100", "owner_id": "7", "channels": [ { "id": "300", "name": "chat" } ],
            "members": [ { "user": { "id": "9", "username": "notch" }, "roles": [] } ] }));
        sh.dispatch("PRESENCE_UPDATE", &json!({ "user": { "id": "9" }, "status": "dnd", "activities": [ { "name": "Minecraft", "url": "https://x.example/live" } ] }));
        sh.lock_state().accounts.link(1, "9").unwrap();
        assert_eq!(value(&sh, Some(1), "user_online_status").as_deref(), Some("dnd"));
        assert_eq!(value(&sh, Some(1), "user_game_name").as_deref(), Some("Minecraft"));
        assert_eq!(value(&sh, Some(1), "user_game_url").as_deref(), Some("https://x.example/live"));
        assert_eq!(value(&sh, None, "linked_online").as_deref(), Some("1"));
        assert_eq!(value(&sh, None, "guild_members_online").as_deref(), Some("1"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
