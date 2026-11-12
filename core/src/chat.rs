//! The chat bridge's two directions: a Discord message in a chat channel as lines for the
//! game's chat (DiscordSRV's `DiscordChatListener`), and a player's chat message as a Discord
//! message (`DiscordSRV.processChatMessage`).
use regex::Regex;
use serde_json::{Value, json};

use crate::config::Chat;
use crate::console::Filters;
use crate::format::{escape_markdown, fill, nearest_color, strip_codes, to_legacy, translate_ampersand, truncate};
use crate::guild::{Cache, Role};

/// What happens with a message from a chat channel.
#[derive(Debug, PartialEq, Eq)]
pub enum FromDiscord {
    /// Not for the game (a bot, a blocked user, filtered away, empty).
    Ignore,
    /// A blocked role (or a missing role of the whitelist): the message gets a cross reaction.
    Rejected,
    /// The author must link first: the message is deleted and this goes to them by PM (nothing
    /// when `DiscordLinkedAccountRequired` is empty).
    LinkRequired(Option<String>),
    /// The `§`-coded lines every player gets: attachments and stickers first, then the text.
    /// `truncated` messages get a speech bubble and an exclamation mark reaction.
    Broadcast { lines: Vec<String>, truncated: bool },
}

/// The compiled filters of the chat side.
pub struct ChatFilters {
    pub game: Filters,
    pub discord: Filters,
}

impl ChatFilters {
    pub fn new(chat: &Chat) -> Result<ChatFilters, String> {
        Ok(ChatFilters {
            game: Filters::new(&chat.discord_chat_channel_game_filters)?,
            discord: Filters::new(&chat.discord_chat_channel_discord_filters)?,
        })
    }
}

fn author_is_bot(d: &Value) -> bool {
    d["author"]["bot"].as_bool().unwrap_or(false)
}

/// Discord's limit for a message's content.
const MAX_CONTENT: usize = 2000;

/// The PM a message of an unlinked author turns into (`DiscordLinkedAccountRequired` with the
/// message in `%message%`, cut to fit Discord's limit).
fn link_required_text(format: &str, message: &str) -> Option<String> {
    if format.is_empty() {
        return None;
    }
    let messageless = format.replace("%message%", "").chars().count();
    if messageless + message.chars().count() <= MAX_CONTENT {
        return Some(format.replace("%message%", message));
    }
    if messageless >= MAX_CONTENT {
        return Some(format.chars().take(MAX_CONTENT).collect());
    }
    let mut room = MAX_CONTENT - messageless;
    let mut shown = message.to_string();
    if room > 3 {
        room -= 3;
        shown = message.chars().take(room).collect::<String>() + "...";
    }
    Some(format.replace("%message%", &shown))
}

/// The icon URL of a sticker of a message (`sticker_items`).
fn sticker_url(item: &Value) -> Option<String> {
    let id = item["id"].as_str()?;
    let extension = match item["format_type"].as_u64() {
        Some(3) => "json",
        Some(4) => "gif",
        _ => "png",
    };
    Some(format!("https://media.discordapp.net/stickers/{id}.{extension}"))
}

/// What a message needs to be put into a chat line.
struct Line<'a> {
    chat: &'a Chat,
    d: &'a Value,
    /// The author's selected roles, highest first.
    selected: Vec<Role>,
    name: String,
    username: String,
    channel_name: String,
    reply: String,
}

impl Line<'_> {
    /// `DiscordToMinecraftChatMessageFormat` (or the no-role one) with `message` in it.
    fn render(&self, message: &str) -> String {
        let chat = self.chat;
        let top = self.selected.first();
        let top_name = top.map(|r| r.name.clone()).unwrap_or_default();
        // Aliases are looked up by role ID, then by lower case role name; keys are lower case.
        let alias = top
            .and_then(|r| {
                let by = |key: &str| chat.discord_chat_channel_role_aliases.iter().find(|(k, _)| k.to_lowercase() == key).map(|(_, v)| v.clone());
                by(&r.id).or_else(|| by(&r.name.to_lowercase()))
            })
            .unwrap_or_else(|| top_name.clone());
        let colour = top.filter(|r| r.color != 0).map(|r| format!("&{}", nearest_color(r.color))).unwrap_or_default();
        let all_roles: Vec<&str> = self.selected.iter().map(|r| r.name.as_str()).filter(|n| !n.trim().is_empty()).collect();
        let template = if self.selected.is_empty() {
            &chat.discord_to_minecraft_chat_message_format_no_role
        } else {
            &chat.discord_to_minecraft_chat_message_format
        };
        // The format's own codes first, then the values (which must not turn into tags).
        let template = to_legacy(&fill(template, &[("toprolecolor", &colour)]));
        fill(
            &template,
            &[
                ("message", message),
                ("name", &self.name),
                ("username", &self.username),
                ("userid", self.d["author"]["id"].as_str().unwrap_or_default()),
                ("channelname", &self.channel_name),
                ("toprolealias", &alias),
                ("toproleinitial", &top_name.chars().next().map(String::from).unwrap_or_default()),
                ("toprole", &top_name),
                ("allroles", &all_roles.join(&chat.discord_to_minecraft_all_roles_separator)),
                ("reply", &self.reply),
            ],
        )
    }
}

/// A MESSAGE_CREATE `d` of a chat channel as the lines players get. `linked` says whether a
/// Discord user ID has a linked account. The order of the checks is `DiscordChatListener`'s.
pub fn discord_to_minecraft(
    chat: &Chat,
    filters: &ChatFilters,
    cache: &Cache,
    d: &Value,
    linked: impl Fn(&str) -> bool,
) -> FromDiscord {
    let author = &d["author"];
    let author_id = author["id"].as_str().unwrap_or_default();
    let webhook = d.get("webhook_id").is_some_and(|w| !w.is_null());
    let bot = author_is_bot(d);
    let content = d["content"].as_str().unwrap_or_default();
    let attachments: Vec<String> = d["attachments"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["url"].as_str()).take(3).map(str::to_string).collect())
        .unwrap_or_default();
    let stickers: Vec<String> = d["sticker_items"].as_array().map(|a| a.iter().filter_map(sticker_url).take(3).collect()).unwrap_or_default();
    if content.trim().is_empty() && attachments.is_empty() && stickers.is_empty() {
        return FromDiscord::Ignore;
    }
    if webhook && chat.discord_chat_channel_block_webhooks {
        return FromDiscord::Ignore;
    }
    if chat.discord_chat_channel_require_linked_account && !bot && !webhook && !linked(author_id) {
        return FromDiscord::LinkRequired(link_required_text(&chat.discord_linked_account_required, content));
    }
    if chat.discord_chat_channel_block_bots && bot && !webhook {
        return FromDiscord::Ignore;
    }
    if chat.discord_chat_channel_blocked_ids.iter().any(|id| id == author_id) {
        return FromDiscord::Ignore;
    }
    let role_ids: Vec<String> = d["member"]["roles"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !webhook {
        let listed = role_ids.iter().any(|r| chat.discord_chat_channel_blocked_roles_ids.contains(r));
        if listed != chat.discord_chat_channel_blocked_roles_as_whitelist {
            return FromDiscord::Rejected;
        }
    }

    let channel_id = d["channel_id"].as_str().unwrap_or_default();
    let guild = cache.guild_of_channel(channel_id);
    // The member's roles, highest first; @everyone is the guild's own ID and never one of them.
    let member_roles: Vec<Role> = if webhook {
        Vec::new()
    } else {
        guild
            .map(|g| cache.sorted_roles(g, &role_ids))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| Some(r.id.as_str()) != guild.map(|g| g.id.as_str()))
            .collect()
    };
    // `DiscordChatChannelRolesSelection` names roles by name or ID (case-sensitive, as a Java list is).
    let selection = &chat.discord_chat_channel_roles_selection;
    let selected: Vec<Role> = member_roles
        .iter()
        .filter(|r| selection.iter().any(|n| *n == r.name || *n == r.id) == chat.discord_chat_channel_roles_selection_as_whitelist)
        .filter(|r| !r.name.trim().is_empty())
        .cloned()
        .collect();
    let colours = &chat.discord_chat_channel_roles_allowed_to_use_color_codes_in_chat;
    let strip = !colours.iter().any(|n| n == "@everyone") && !member_roles.iter().any(|r| colours.iter().any(|n| *n == r.name || *n == r.id));

    let username = strip_codes(author["username"].as_str().unwrap_or("?"));
    let name = strip_codes(d["member"]["nick"].as_str().unwrap_or(&username));
    let line = Line {
        chat,
        d,
        selected,
        name,
        username,
        channel_name: cache.channel_name(channel_id).unwrap_or_default().to_string(),
        reply: reply_text(chat, cache, d),
    };

    let mut lines = Vec::new();
    for url in attachments.iter().chain(stickers.iter()) {
        if let Some(url) = filters.discord.apply(url) {
            lines.push(line.render(&url));
        }
    }
    if content.trim().is_empty() {
        return FromDiscord::Broadcast { lines, truncated: false };
    }
    let Some(message) = filters.discord.apply(&content.replace('\u{1b}', "")) else {
        return FromDiscord::Broadcast { lines, truncated: false };
    };
    let limit = chat.discord_chat_channel_truncate_length;
    let truncated = limit > 0 && message.chars().count() > limit;
    let message = truncate(&message, limit);
    let message = if strip { strip_codes(&message) } else { translate_ampersand(&message) };
    let message = convert_text(chat, cache, d, &message);
    let hide_emoji = chat.discord_chat_channel_emoji_behavior.eq_ignore_ascii_case("hide");
    let message = if hide_emoji { message.chars().filter(|c| !is_emoji(*c)).collect::<String>() } else { message };
    if !message.trim().is_empty() {
        lines.push(line.render(&message));
    }
    FromDiscord::Broadcast { lines, truncated }
}

/// The replied-to message as `DiscordToMinecraftMessageReplyFormat` (empty for no reply).
fn reply_text(chat: &Chat, cache: &Cache, d: &Value) -> String {
    let r = &d["referenced_message"];
    if !r.is_object() {
        return String::new();
    }
    let username = r["author"]["username"].as_str().unwrap_or_default();
    let id = r["author"]["id"].as_str().unwrap_or_default();
    let name = r["member"]["nick"]
        .as_str()
        .map(str::to_string)
        .or_else(|| cache.members.get(id).map(|m| m.effective_name().to_string()))
        .unwrap_or_else(|| username.to_string());
    let message = convert_text(chat, cache, r, r["content"].as_str().unwrap_or_default());
    fill(
        &chat.discord_to_minecraft_message_reply_format,
        &[("name", &strip_codes(&name)), ("username", &strip_codes(username)), ("userid", id), ("message", &strip_codes(&message))],
    )
}

/// The message's text as players should read it: mentions as names (a mention of someone, a
/// channel or a role the bot doesn't know vanishes, as in DiscordSRV's renderer), custom emotes
/// as `:name:` (or gone with `DiscordChatChannelEmoteBehavior` other than `name`).
fn convert_text(chat: &Chat, cache: &Cache, d: &Value, text: &str) -> String {
    let user = Regex::new(r"<@!?(\d{16,20})>").expect("static regex");
    let text = user.replace_all(text, |c: &regex::Captures| {
        let id = &c[1];
        d["mentions"]
            .as_array()
            .and_then(|m| m.iter().find(|u| u["id"] == id))
            .and_then(|u| u["member"]["nick"].as_str().or(u["username"].as_str()))
            .map(str::to_string)
            .or_else(|| cache.members.get(id).map(|m| m.effective_name().to_string()))
            .map(|n| format!("@{n}"))
            .unwrap_or_default()
    });
    let channel = Regex::new(r"<#(\d{16,20})>").expect("static regex");
    let text = channel.replace_all(&text, |c: &regex::Captures| cache.channel_name(&c[1]).map(|n| format!("#{n}")).unwrap_or_default());
    let role = Regex::new(r"<@&(\d{16,20})>").expect("static regex");
    let guild = cache.guild_of_channel(d["channel_id"].as_str().unwrap_or_default());
    let text = role.replace_all(&text, |c: &regex::Captures| {
        guild.and_then(|g| g.roles.get(&c[1])).map(|r| format!("@{}", r.name)).unwrap_or_default()
    });
    let emote = Regex::new(r"<a?:([a-zA-Z]{2,32}):\d{16,20}>").expect("static regex");
    let show_emotes = chat.discord_chat_channel_emote_behavior.eq_ignore_ascii_case("name");
    emote.replace_all(&text, if show_emotes { ":$1:" } else { "" }).into_owned()
}

/// The text of the message (mentions and emotes read), for callers that only have the payload.
pub fn content_for_minecraft(chat: &Chat, cache: &Cache, d: &Value) -> String {
    convert_text(chat, cache, d, d["content"].as_str().unwrap_or_default()).trim().to_string()
}

/// Pictographs and the joiners/selectors emojis are built of (for the `hide` behaviour).
fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F | 0x200D | 0x20E3)
}

/// A player's chat message, for the Discord side.
pub struct GameChat<'a> {
    pub username: &'a str,
    pub display_name: &'a str,
    pub message: &'a str,
    pub primary_group: &'a str,
    pub world: &'a str,
    pub date: &'a str,
    /// The game channel (`global`); `%channelname%` shows it with a capital.
    pub channel: &'a str,
}

/// A player's chat message as the content of a Discord message, None when the prefix rule or a
/// filter keeps it in the game.
pub fn minecraft_to_discord(chat: &Chat, filters: &ChatFilters, cache: &Cache, m: &GameChat) -> Option<String> {
    let message = strip_codes(m.message);
    // As in DiscordSRV, an empty prefix with `PrefixActsAsBlacklist` keeps everything in the game.
    if message.starts_with(chat.discord_chat_channel_prefix_required_to_process_message.as_str())
        == chat.discord_chat_channel_prefix_acts_as_blacklist
    {
        return None;
    }
    let message = if chat.discord_chat_channel_translate_mentions {
        translate_mentions(chat, cache, &message)
    } else {
        message.replace('@', "@\u{200B}")
    };
    let template = if m.primary_group.is_empty() {
        &chat.minecraft_chat_to_discord_message_format_no_primary_group
    } else {
        &chat.minecraft_chat_to_discord_message_format
    };
    let username = strip_codes(m.username);
    let display = strip_codes(m.display_name);
    let mut channel = m.channel.chars();
    let channel = channel.next().map(|f| f.to_uppercase().collect::<String>() + channel.as_str()).unwrap_or_default();
    let pattern = fill(
        template,
        &[
            ("usernamenoescapes", &username),
            ("displaynamenoescapes", &display),
            ("username", &escape_markdown(&username)),
            ("displayname", &escape_markdown(&display)),
            ("primarygroup", m.primary_group),
            ("worldalias", m.world),
            ("world", m.world),
            ("date", m.date),
            ("time", m.date),
            ("channelname", &channel),
            // Put back: the message goes in after the colour codes of the rest are stripped.
            ("message", "%message%"),
        ],
    );
    let text = strip_codes(&pattern).replace("%message%", &message);
    let text = filters.game.apply(&text)?;
    Some(truncate(&text, crate::console::MAX_MESSAGE))
}

/// `@name` in a game message as a Discord mention of the role or member with that name (case
/// insensitive, not right after a `<`), for the roles and members the bot knows.
pub fn translate_mentions(chat: &Chat, cache: &Cache, message: &str) -> String {
    if !message.contains('@') {
        return message.to_string();
    }
    let Some(guild) = chat.channel_for("global").and_then(|c| cache.guild_of_channel(c)) else {
        return message.to_string();
    };
    let mut entries: Vec<(String, String)> = guild.roles.values().map(|r| (r.name.clone(), format!("<@&{}>", r.id))).collect();
    entries.extend(cache.members.iter().map(|(id, m)| (m.effective_name().to_string(), format!("<@{id}>"))));
    entries.retain(|(name, _)| !name.is_empty());
    entries.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()).then(a.1.cmp(&b.1)));
    let mut text = message.to_string();
    for (name, mention) in &entries {
        text = replace_mention(&text, name, mention);
    }
    text
}

/// `@name` (any case) replaced by `mention`, unless a `<` comes right before it.
fn replace_mention(text: &str, name: &str, mention: &str) -> String {
    let needle: Vec<char> = format!("@{name}").chars().collect();
    let hay: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < hay.len() {
        let end = i + needle.len();
        let hit = end <= hay.len()
            && (i == 0 || hay[i - 1] != '<')
            && hay[i..end].iter().zip(&needle).all(|(a, b)| a == b || a.to_lowercase().eq(b.to_lowercase()));
        if hit {
            out.push_str(mention);
            i = end;
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// `allowed_mentions` for `DiscordChatChannelAllowedMentions`.
pub fn allowed_mentions(chat: &Chat) -> Value {
    let mut parse = Vec::new();
    if chat.mention_allowed("user") {
        parse.push("users");
    }
    if chat.mention_allowed("role") {
        parse.push("roles");
    }
    if chat.mention_allowed("everyone") || chat.mention_allowed("here") {
        parse.push("everyone");
    }
    json!({ "parse": parse })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DEFAULT_CONFIG};
    use crate::guild::tests::sample;

    fn chat() -> Chat {
        let mut c = Config::parse(DEFAULT_CONFIG).unwrap().chat;
        c.channels = vec![("global".into(), "300".into())];
        c
    }

    fn message(content: &str, roles: &[&str]) -> Value {
        json!({
            "channel_id": "300", "guild_id": "100", "content": content,
            "author": { "id": "9", "username": "notch", "global_name": "Notch" },
            "member": { "nick": null, "roles": roles },
        })
    }

    fn lines(got: FromDiscord) -> Vec<String> {
        match got {
            FromDiscord::Broadcast { lines, .. } => lines,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_member_with_roles_gets_the_role_format() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let got = discord_to_minecraft(&c, &f, &sample(), &message("hello", &["202", "203"]), |_| true);
        // Developer (Misc role is filtered by RolesSelection), aliased to Dev, #3498DB ~ dark aqua.
        assert_eq!(lines(got), vec!["[§bDiscord§r | §3Dev§r] notch » hello".to_string()]);
    }

    #[test]
    fn a_member_without_roles_gets_the_plain_format() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let got = discord_to_minecraft(&c, &f, &sample(), &message("&chi", &[]), |_| true);
        assert_eq!(lines(got), vec!["[§bDiscord§r] notch » hi".to_string()]);
    }

    #[test]
    fn colour_codes_need_a_listed_role() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let got = discord_to_minecraft(&c, &f, &sample(), &message("&cred", &["201"]), |_| true);
        assert_eq!(lines(got), vec!["[§bDiscord§r | §cOwner§r] notch » §cred".to_string()]);
    }

    #[test]
    fn aliases_are_found_by_role_id_before_the_lower_case_name() {
        let mut c = chat();
        c.discord_chat_channel_role_aliases.insert("201".into(), "Boss".into());
        let f = ChatFilters::new(&c).unwrap();
        let got = discord_to_minecraft(&c, &f, &sample(), &message("x", &["201", "202"]), |_| true);
        assert_eq!(lines(got), vec!["[§bDiscord§r | §cBoss§r] notch » x".to_string()]);
    }

    #[test]
    fn webhooks_blocked_ids_roles_filters_and_linking() {
        let mut c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let mut hook = message("x", &[]);
        hook["webhook_id"] = json!("5");
        assert_eq!(discord_to_minecraft(&c, &f, &sample(), &hook, |_| true), FromDiscord::Ignore);
        // A filter that empties the message leaves nothing to say.
        let filtered = message("**Online players (2):**", &[]);
        assert!(lines(discord_to_minecraft(&c, &f, &sample(), &filtered, |_| true)).is_empty());
        c.discord_chat_channel_blocked_ids = vec!["9".into()];
        assert_eq!(discord_to_minecraft(&c, &f, &sample(), &message("x", &[]), |_| true), FromDiscord::Ignore);
        c.discord_chat_channel_blocked_ids.clear();
        c.discord_chat_channel_blocked_roles_ids = vec!["202".into()];
        assert_eq!(discord_to_minecraft(&c, &f, &sample(), &message("x", &["202"]), |_| true), FromDiscord::Rejected);
        c.discord_chat_channel_blocked_roles_ids.clear();
        c.discord_chat_channel_require_linked_account = true;
        let got = discord_to_minecraft(&c, &f, &sample(), &message("secret", &[]), |_| false);
        assert!(matches!(got, FromDiscord::LinkRequired(Some(t)) if t.contains("```secret```")));
        let mut bot = message("secret", &[]);
        bot["author"]["bot"] = json!(true);
        assert!(matches!(discord_to_minecraft(&c, &f, &sample(), &bot, |_| false), FromDiscord::Broadcast { .. }));
    }

    #[test]
    fn the_link_required_pm_is_cut_to_fit() {
        let long = "x".repeat(3000);
        let text = link_required_text("Link first: %message%", &long).unwrap();
        assert_eq!(text.chars().count(), 2000);
        assert!(text.ends_with("..."));
        assert_eq!(link_required_text("", "hi"), None);
    }

    #[test]
    fn long_messages_are_truncated_and_flagged() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let FromDiscord::Broadcast { lines, truncated } =
            discord_to_minecraft(&c, &f, &sample(), &message(&"a".repeat(400), &[]), |_| true)
        else {
            panic!()
        };
        assert!(truncated);
        assert_eq!(lines[0].matches('a').count(), 256);
    }

    #[test]
    fn attachments_and_stickers_are_lines_of_their_own_before_the_text() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let mut d = message("look", &[]);
        d["attachments"] = json!([{ "url": "https://cdn.example/a.png" }, { "url": "https://cdn.example/b.png" },
                                  { "url": "https://cdn.example/c.png" }, { "url": "https://cdn.example/d.png" }]);
        d["sticker_items"] = json!([{ "id": "5", "format_type": 4 }]);
        let got = lines(discord_to_minecraft(&c, &f, &sample(), &d, |_| true));
        assert_eq!(got.len(), 5);
        assert!(got[0].ends_with("» https://cdn.example/a.png"));
        assert!(got[2].ends_with("» https://cdn.example/c.png"));
        assert!(got[3].ends_with("» https://media.discordapp.net/stickers/5.gif"));
        assert!(got[4].ends_with("» look"));
        let mut only = message("", &[]);
        only["attachments"] = json!([{ "url": "https://cdn.example/a.png" }]);
        assert_eq!(lines(discord_to_minecraft(&c, &f, &sample(), &only, |_| true)).len(), 1);
    }

    #[test]
    fn mentions_and_emotes_read_as_text_and_unknown_ones_vanish() {
        let c = chat();
        let mut cache = sample();
        cache.apply(
            "GUILD_CREATE",
            &json!({
                "id": "100", "owner_id": "7",
                "roles": [ { "id": "201000000000000001", "name": "Owner", "color": 0, "position": 5 } ],
                "channels": [ { "id": "301000000000000001", "name": "rules" } ],
            }),
        );
        let d = json!({
            "channel_id": "300",
            "content": "<@420000000000000042> see <#301000000000000001> <@&201000000000000001> <:pog:123456789012345678> <@999999999999999999> <#999999999999999999>",
            "mentions": [ { "id": "420000000000000042", "username": "jeb_" } ],
        });
        assert_eq!(content_for_minecraft(&c, &cache, &d), "@jeb_ see #rules @Owner :pog:");
    }

    #[test]
    fn replies_show_the_replied_name() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let mut d = message("yes", &[]);
        d["referenced_message"] = json!({ "content": "q", "author": { "id": "3", "username": "alex" } });
        assert_eq!(lines(discord_to_minecraft(&c, &f, &sample(), &d, |_| true)), vec!["[§bDiscord§r] notch (replying to alex) » yes".to_string()]);
    }

    fn game<'a>(message: &'a str, group: &'a str) -> GameChat<'a> {
        GameChat { username: "jeb_", display_name: "jeb_", message, primary_group: group, world: "world", date: "", channel: "global" }
    }

    #[test]
    fn game_chat_goes_out_escaped_without_a_group() {
        let c = chat();
        let f = ChatFilters::new(&c).unwrap();
        let mut cache = sample();
        cache.apply("MESSAGE_CREATE", &json!({ "author": { "id": "9", "username": "notch" } }));
        let m = game("hi @notch, read #rules §cnow", "");
        // Only roles and members are turned into mentions (not channels).
        assert_eq!(minecraft_to_discord(&c, &f, &cache, &m).unwrap(), "jeb\\_ » hi <@9>, read #rules now");
        assert!(minecraft_to_discord(&c, &f, &cache, &game("x", "admin")).unwrap().starts_with("**admin** jeb\\_ » "));
        let role = game("ping @owner", "");
        assert_eq!(minecraft_to_discord(&c, &f, &cache, &role).unwrap(), "jeb\\_ » ping <@&201>");
    }

    #[test]
    fn mentions_are_defused_when_translation_is_off() {
        let mut c = chat();
        c.discord_chat_channel_translate_mentions = false;
        let f = ChatFilters::new(&c).unwrap();
        let got = minecraft_to_discord(&c, &f, &sample(), &game("@everyone", "")).unwrap();
        assert_eq!(got, "jeb\\_ » @\u{200B}everyone");
    }

    #[test]
    fn game_filters_see_the_whole_formatted_text() {
        let mut c = chat();
        c.discord_chat_channel_game_filters = vec![("secret".into(), "[hidden]".into())];
        let f = ChatFilters::new(&c).unwrap();
        let got = minecraft_to_discord(&c, &f, &sample(), &game("a secret", "")).unwrap();
        assert_eq!(got, "jeb\\_ » a [hidden]");
        c.discord_chat_channel_game_filters = vec![(".*".into(), "".into())];
        let f = ChatFilters::new(&c).unwrap();
        assert!(minecraft_to_discord(&c, &f, &sample(), &game("a", "")).is_none());
    }

    #[test]
    fn the_prefix_rule_keeps_messages_in_game() {
        let mut c = chat();
        c.discord_chat_channel_prefix_required_to_process_message = "!".into();
        let f = ChatFilters::new(&c).unwrap();
        let m = |message| GameChat { username: "a", display_name: "a", message, primary_group: "", world: "", date: "", channel: "global" };
        assert!(minecraft_to_discord(&c, &f, &Cache::default(), &m("hi")).is_none());
        assert!(minecraft_to_discord(&c, &f, &Cache::default(), &m("!hi")).is_some());
        c.discord_chat_channel_prefix_acts_as_blacklist = true;
        assert!(minecraft_to_discord(&c, &f, &Cache::default(), &m("!hi")).is_none());
    }

    #[test]
    fn allowed_mentions_follow_the_list() {
        assert_eq!(allowed_mentions(&chat()), json!({ "parse": ["users"] }));
    }
}
