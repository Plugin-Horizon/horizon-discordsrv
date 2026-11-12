//! messages.yml's notification templates (join, first join, leave, death, advancement) as the
//! body of a Discord message: content and one embed, placeholders filled.
use chrono::{TimeZone, Utc};
use serde_json::{Map, Value, json};

use crate::config::{MessageTemplate, Timestamp};
use crate::format::{fill, truncate};

/// Discord's limits: author name, title, description, field name, field value, footer.
const AUTHOR: usize = 256;
const TITLE: usize = 256;
const DESCRIPTION: usize = 4096;
const FIELD_NAME: usize = 256;
const FIELD_VALUE: usize = 1024;
const FOOTER: usize = 2048;

/// `#rrggbb`, `rrggbb` or a decimal integer.
pub fn parse_color(color: &str) -> Option<u32> {
    let c = color.trim();
    if let Some(hex) = c.strip_prefix('#') {
        return u32::from_str_radix(hex, 16).ok();
    }
    c.parse::<u32>().ok().or_else(|| (c.len() == 6).then(|| u32::from_str_radix(c, 16).ok()).flatten())
}

/// The message body for a template, None when it is off or would be empty. `now` is epoch
/// seconds (for `Timestamp = true`).
pub fn render(template: &MessageTemplate, pairs: &[(&str, &str)], now: i64) -> Option<Value> {
    if !template.enabled {
        return None;
    }
    let f = |s: &str| fill(s, pairs).trim().to_string();
    let mut body = Map::new();
    let content = f(&template.content);
    if !content.is_empty() {
        body.insert("content".into(), Value::String(truncate(&content, crate::console::MAX_MESSAGE)));
    }
    let e = &template.embed;
    if e.enabled {
        let mut embed = Map::new();
        let mut put = |key: &str, value: Value| {
            embed.insert(key.into(), value);
        };
        if let Some(c) = parse_color(&e.color) {
            put("color", json!(c & 0xFF_FFFF));
        }
        let author_name = f(&e.author.name);
        if !author_name.is_empty() {
            let mut a = Map::new();
            a.insert("name".into(), json!(truncate(&author_name, AUTHOR)));
            let (icon, url) = (f(&e.author.image_url), f(&e.author.url));
            if is_url(&icon) {
                a.insert("icon_url".into(), json!(icon));
            }
            if is_url(&url) {
                a.insert("url".into(), json!(url));
            }
            put("author", Value::Object(a));
        }
        let title = f(&e.title.text);
        if !title.is_empty() {
            put("title", json!(truncate(&title, TITLE)));
            let url = f(&e.title.url);
            if is_url(&url) {
                put("url", json!(url));
            }
        }
        let description = f(&e.description);
        if !description.is_empty() {
            put("description", json!(truncate(&description, DESCRIPTION)));
        }
        let fields: Vec<Value> = e.fields.iter().filter_map(|x| field(&f(x))).collect();
        if !fields.is_empty() {
            put("fields", Value::Array(fields));
        }
        for (key, url) in [("thumbnail", f(&e.thumbnail_url)), ("image", f(&e.image_url))] {
            if is_url(&url) {
                put(key, json!({ "url": url }));
            }
        }
        let footer = f(&e.footer.text);
        if !footer.is_empty() {
            let mut ft = Map::new();
            ft.insert("text".into(), json!(truncate(&footer, FOOTER)));
            let icon = f(&e.footer.icon_url);
            if is_url(&icon) {
                ft.insert("icon_url".into(), json!(icon));
            }
            put("footer", Value::Object(ft));
        }
        let at = match e.timestamp {
            Timestamp::Now(true) => Some(now),
            Timestamp::Epoch(s) => Some(s),
            Timestamp::Now(false) => None,
        };
        if let Some(t) = at.and_then(|s| Utc.timestamp_opt(s, 0).single()) {
            put("timestamp", json!(t.to_rfc3339()));
        }
        // An embed of a colour or a timestamp alone is not something Discord shows.
        if embed.keys().any(|k| k != "color" && k != "timestamp") {
            body.insert("embeds".into(), json!([embed]));
        }
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// `title;value;inline` or `blank`.
fn field(spec: &str) -> Option<Value> {
    if spec.eq_ignore_ascii_case("blank") {
        return Some(json!({ "name": "\u{200b}", "value": "\u{200b}", "inline": true }));
    }
    let mut parts = spec.splitn(3, ';');
    let name = parts.next()?.trim();
    let value = parts.next()?.trim();
    let inline = parts.next().is_some_and(|i| i.trim().eq_ignore_ascii_case("true"));
    if name.is_empty() && value.is_empty() {
        return None;
    }
    let or_blank = |s: &str, max| if s.is_empty() { "\u{200b}".to_string() } else { truncate(s, max) };
    Some(json!({ "name": or_blank(name, FIELD_NAME), "value": or_blank(value, FIELD_VALUE), "inline": inline }))
}

/// The avatar URL pattern of `AvatarUrl` for a player.
pub fn avatar_url(pattern: &str, username: &str, uuid: u128, size: u32) -> String {
    let dashed = crate::server::uuid_string(uuid);
    let pattern = if pattern.trim().is_empty() { crate::config::DEFAULT_AVATAR_URL } else { pattern };
    pattern
        .replace("{username}", username)
        .replace("{uuid-nodashes}", &dashed.replace('-', ""))
        .replace("{uuid}", &dashed)
        .replace("{size}", &size.to_string())
        // The skin texture's hash is not known to a mod (GAPS.md): the UUID stands in.
        .replace("{texture}", &dashed.replace('-', ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DEFAULT_CONFIG};

    #[test]
    fn the_default_join_message_is_a_green_author_embed() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap().chat;
        let body = render(
            &c.minecraft_player_join_message,
            &[("username", "jeb_"), ("embedavatarurl", "https://a.example/jeb.png")],
            0,
        )
        .unwrap();
        assert_eq!(
            body,
            json!({ "embeds": [ { "color": 0x00ff00, "author": { "name": "jeb_ joined the server", "icon_url": "https://a.example/jeb.png" } } ] })
        );
    }

    #[test]
    fn fields_footer_timestamp_and_content() {
        let mut t = MessageTemplate { enabled: true, content: "%x% happened".into(), ..Default::default() };
        t.embed.enabled = true;
        t.embed.title.text = "T".into();
        t.embed.fields = vec!["Who;%x%;true".into(), "blank".into(), ";".into()];
        t.embed.footer.text = "f".into();
        t.embed.timestamp = Timestamp::Now(true);
        let body = render(&t, &[("x", "jeb")], 1_700_000_000).unwrap();
        assert_eq!(body["content"], "jeb happened");
        let e = &body["embeds"][0];
        assert_eq!(e["fields"][0], json!({ "name": "Who", "value": "jeb", "inline": true }));
        assert_eq!(e["fields"].as_array().unwrap().len(), 2);
        assert_eq!(e["footer"]["text"], "f");
        assert_eq!(e["timestamp"], "2023-11-14T22:13:20+00:00");
    }

    #[test]
    fn disabled_or_empty_templates_send_nothing() {
        let c = Config::parse(DEFAULT_CONFIG).unwrap().chat;
        let mut t = c.minecraft_player_death_message.clone();
        assert!(render(&t, &[("deathmessage", "")], 0).is_none());
        t.enabled = false;
        assert!(render(&t, &[("deathmessage", "x")], 0).is_none());
    }

    #[test]
    fn colours_and_avatars() {
        assert_eq!(parse_color("#ffd700"), Some(0xffd700));
        assert_eq!(parse_color("0"), Some(0));
        assert_eq!(parse_color("nope"), None);
        let u = 0x0123456789abcdef0123456789abcdefu128;
        assert_eq!(
            avatar_url("", "Notch", u, 128),
            "https://crafatar.com/avatars/0123456789abcdef0123456789abcdef?size=128&overlay"
        );
    }
}
