//! Console lines: heard from the log hook (`feed`), filtered, formatted with the
//! configured prefix and packed into Discord messages.
use chrono::{DateTime, Local};
use regex::Regex;

use crate::config::Config;

/// Discord's limit for a message's `content`.
pub const MAX_MESSAGE: usize = 2000;

/// One line of the log: `[12:04:31] [Server thread/INFO]: message`.
#[derive(Debug, PartialEq, Eq)]
pub struct LogLine {
    pub level: String,
    pub message: String,
}

/// Splits a log line. A line without the pattern (the rest of a multi-line message) keeps the
/// level of the line before it, `last_level`.
pub fn parse_line(line: &str, last_level: &str) -> LogLine {
    let parsed = (|| {
        let rest = line.strip_prefix('[')?;
        let (_, rest) = rest.split_once("] [")?;
        let (thread_level, message) = rest.split_once("]: ")?;
        let (_, level) = thread_level.rsplit_once('/')?;
        Some(LogLine { level: level.to_string(), message: message.to_string() })
    })();
    parsed.unwrap_or_else(|| LogLine { level: last_level.to_string(), message: line.to_string() })
}

/// The compiled `DiscordConsoleChannelFilters`.
pub struct Filters(Vec<(Regex, String)>);

impl Filters {
    pub fn new(pairs: &[(String, String)]) -> Result<Filters, String> {
        pairs
            .iter()
            .map(|(p, r)| Regex::new(p).map(|re| (re, java_replacement(r))).map_err(|e| format!("filter {p:?}: {e}")))
            .collect::<Result<_, _>>()
            .map(Filters)
    }

    /// The line after every filter, or None when it ended up empty.
    pub fn apply(&self, message: &str) -> Option<String> {
        let mut text = message.to_string();
        for (re, rep) in &self.0 {
            text = re.replace_all(&text, rep.as_str()).into_owned();
        }
        (!text.trim().is_empty()).then_some(text)
    }
}

/// Java's `$1` group references are Rust's `${1}` (so `$1abc` is not read as group `1abc`).
fn java_replacement(r: &str) -> String {
    let mut out = String::new();
    let mut chars = r.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('$') => out.push_str("$$"),
                Some(n) => out.push(n),
                None => {}
            },
            '$' if chars.peek().is_some_and(|n| n.is_ascii_digit()) => {
                let mut num = String::new();
                while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    num.push(*d);
                    chars.next();
                }
                out.push_str(&format!("${{{num}}}"));
            }
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out
}

/// Minecraft's `§x` colour and format codes, removed.
pub fn strip_colors(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// A Java `SimpleDateFormat` pattern (the letters DiscordSRV's configs use) as a chrono format.
pub fn java_date_format(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            if chars.get(i) == Some(&'\'') {
                out.push('\'');
                i += 1;
                continue;
            }
            loop {
                match (chars.get(i), chars.get(i + 1)) {
                    (None, _) => break,
                    // '' inside quoted text is one quote.
                    (Some('\''), Some('\'')) => {
                        out.push('\'');
                        i += 2;
                    }
                    (Some('\''), _) => break,
                    (Some(c), _) => {
                        push_literal(&mut out, *c);
                        i += 1;
                    }
                }
            }
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let spec = match (c, n) {
            ('y', 2) => "%y",
            ('y', _) => "%Y",
            ('M', 1) => "%-m",
            ('M', 2) => "%m",
            ('M', 3) => "%b",
            ('M', _) => "%B",
            ('d', 1) => "%-d",
            ('d', _) => "%d",
            ('E', 1..=3) => "%a",
            ('E', _) => "%A",
            ('H', 1) => "%-H",
            ('H', _) => "%H",
            ('h', 1) => "%-I",
            ('h', _) => "%I",
            ('m', 1) => "%-M",
            ('m', _) => "%M",
            ('s', 1) => "%-S",
            ('s', _) => "%S",
            ('S', _) => "%3f",
            ('a', _) => "%p",
            ('z', _) => "%Z",
            ('Z', _) => "%z",
            _ => {
                for _ in 0..n {
                    push_literal(&mut out, c);
                }
                i += n;
                continue;
            }
        };
        out.push_str(spec);
        i += n;
    }
    out
}

fn push_literal(out: &mut String, c: char) {
    if c == '%' {
        out.push_str("%%");
    } else {
        out.push(c);
    }
}

/// Formats console lines as `DiscordConsoleChannelPrefix` + line + `DiscordConsoleChannelSuffix`.
pub struct Formatter {
    filters: Filters,
    date_format: String,
    prefix: String,
    suffix: String,
}

impl Formatter {
    pub fn new(config: &Config) -> Result<Formatter, String> {
        Ok(Formatter {
            filters: Filters::new(&config.discord_console_channel_filters)?,
            date_format: java_date_format(&config.discord_console_channel_timestamp_format),
            prefix: config.discord_console_channel_prefix.clone(),
            suffix: config.discord_console_channel_suffix.clone(),
        })
    }

    pub fn format(&self, line: &LogLine, now: &DateTime<Local>) -> Option<String> {
        let message = self.filters.apply(&strip_colors(&line.message))?;
        let fill = |s: &str| {
            s.replace("{date}", &now.format(&self.date_format).to_string())
                .replace("{datetime}", &now.format("%a %b %-d %H:%M:%S %Z %Y").to_string())
                .replace("{level}", &line.level)
                // The Java logger's name; Horizon's log has none (see README).
                .replace("{name}", "")
        };
        Some(format!("{}{}{}", fill(&self.prefix), message, fill(&self.suffix)))
    }
}

/// Packs lines into as few messages of at most [`MAX_MESSAGE`] characters as they fit in,
/// each inside a code block when `code_blocks` is on. A line too long on its own is cut.
pub fn pack(lines: &[String], code_blocks: bool) -> Vec<String> {
    let (open, close) = if code_blocks { ("```\n", "\n```") } else { ("", "") };
    let room = MAX_MESSAGE - open.len() - close.len();
    let mut messages = Vec::new();
    let mut body = String::new();
    for line in lines {
        let mut line = if code_blocks { line.replace("```", "`\u{200b}``") } else { line.clone() };
        if line.chars().count() > room {
            line = line.chars().take(room).collect();
        }
        let needed = if body.is_empty() { line.chars().count() } else { body.chars().count() + 1 + line.chars().count() };
        if needed > room && !body.is_empty() {
            messages.push(format!("{open}{body}{close}"));
            body.clear();
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&line);
    }
    if !body.is_empty() {
        messages.push(format!("{open}{body}{close}"));
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn log_lines_split_into_level_and_message() {
        assert_eq!(
            parse_line("[09:05:07] [Server thread/INFO]: Done (1.2s)!", "INFO"),
            LogLine { level: "INFO".into(), message: "Done (1.2s)!".into() }
        );
        assert_eq!(
            parse_line("[09:05:07] [User Authenticator #1/WARN]: x: y", "INFO"),
            LogLine { level: "WARN".into(), message: "x: y".into() }
        );
        assert_eq!(
            parse_line("\tat foo.bar(Baz.java:1)", "ERROR"),
            LogLine { level: "ERROR".into(), message: "\tat foo.bar(Baz.java:1)".into() }
        );
    }

    #[test]
    fn example_filters_hide_ips_and_stack_traces() {
        let c = Config::parse(crate::config::DEFAULT_CONFIG).unwrap();
        let f = Filters::new(&c.discord_console_channel_filters).unwrap();
        assert_eq!(f.apply("Alex[/10.1.2.3:51234] logged in").unwrap(), "Alex[/<ip>] logged in");
        assert_eq!(f.apply("    at net.minecraft.Foo.bar(Foo.java:1)"), None);
        assert_eq!(f.apply("  ... 12 more"), None);
        assert_eq!(f.apply("Entity uuid already exists: abc"), None);
        assert_eq!(f.apply("Done (1.2s)!").unwrap(), "Done (1.2s)!");
    }

    #[test]
    fn java_group_references_work() {
        let f = Filters::new(&[("(\\w+) joined".into(), "$1 arrived".into())]).unwrap();
        assert_eq!(f.apply("Alex joined").unwrap(), "Alex arrived");
    }

    #[test]
    fn java_date_patterns_become_chrono_formats() {
        assert_eq!(java_date_format("EEE HH:mm:ss"), "%a %H:%M:%S");
        assert_eq!(java_date_format("EEE, d. MMM yyyy HH:mm:ss z"), "%a, %-d. %b %Y %H:%M:%S %Z");
        assert_eq!(java_date_format("yyyy-MM-dd"), "%Y-%m-%d");
        assert_eq!(java_date_format("'at' HH 'o''clock' 100%"), "at %H o'clock 100%%");
    }

    #[test]
    fn lines_get_the_example_prefix() {
        let c = Config::parse(crate::config::DEFAULT_CONFIG).unwrap();
        let f = Formatter::new(&c).unwrap();
        let now = Local.with_ymd_and_hms(2026, 11, 5, 9, 5, 7).unwrap();
        let line = parse_line("[09:05:07] [Server thread/INFO]: §aDone§r (1.2s)!", "INFO");
        assert_eq!(f.format(&line, &now).unwrap(), "[Thu 09:05:07 INFO] Done (1.2s)!");
        let trace = parse_line("\tat a.b(C.java:1)", "ERROR");
        assert_eq!(f.format(&trace, &now), None);
    }

    #[test]
    fn packing_keeps_messages_under_discords_limit() {
        let lines: Vec<String> = (0..300).map(|i| format!("line {i:03} {}", "x".repeat(20))).collect();
        let messages = pack(&lines, true);
        assert!(messages.len() > 1);
        for m in &messages {
            assert!(m.chars().count() <= MAX_MESSAGE, "{}", m.len());
            assert!(m.starts_with("```\n") && m.ends_with("\n```"));
        }
        let joined: Vec<&str> = messages
            .iter()
            .flat_map(|m| m.trim_start_matches("```\n").trim_end_matches("\n```").lines())
            .collect();
        assert_eq!(joined.len(), 300);
        assert_eq!(pack(&["a".repeat(5000)], false)[0].chars().count(), MAX_MESSAGE);
        assert_eq!(pack(&["a ``` b".to_string()], true)[0], "```\na `\u{200b}`` b\n```");
        assert!(pack(&[], true).is_empty());
    }
}
