//! Text helpers shared by the chat side: MiniMessage tags and `&` codes as Minecraft's `§`
//! codes (what the server's `Text::legacy` reads), colour approximation, Discord markdown
//! escaping and `%placeholder%` filling.

/// The sixteen chat colours: code, MiniMessage name, RGB.
pub const COLORS: [(char, &str, u32); 16] = [
    ('0', "black", 0x000000),
    ('1', "dark_blue", 0x0000AA),
    ('2', "dark_green", 0x00AA00),
    ('3', "dark_aqua", 0x00AAAA),
    ('4', "dark_red", 0xAA0000),
    ('5', "dark_purple", 0xAA00AA),
    ('6', "gold", 0xFFAA00),
    ('7', "gray", 0xAAAAAA),
    ('8', "dark_gray", 0x555555),
    ('9', "blue", 0x5555FF),
    ('a', "green", 0x55FF55),
    ('b', "aqua", 0x55FFFF),
    ('c', "red", 0xFF5555),
    ('d', "light_purple", 0xFF55FF),
    ('e', "yellow", 0xFFFF55),
    ('f', "white", 0xFFFFFF),
];

/// The chat colour closest to `rgb` (Discord role colours have no exact counterpart).
pub fn nearest_color(rgb: u32) -> char {
    let (r, g, b) = ((rgb >> 16) as i64 & 255, (rgb >> 8) as i64 & 255, rgb as i64 & 255);
    COLORS
        .iter()
        .min_by_key(|(_, _, c)| {
            let (cr, cg, cb) = ((c >> 16) as i64 & 255, (c >> 8) as i64 & 255, *c as i64 & 255);
            (r - cr).pow(2) + (g - cg).pow(2) + (b - cb).pow(2)
        })
        .map_or('f', |c| c.0)
}

fn decoration(tag: &str) -> Option<char> {
    Some(match tag {
        "bold" | "b" => 'l',
        "italic" | "i" | "em" => 'o',
        "underlined" | "u" => 'n',
        "strikethrough" | "st" => 'm',
        "obfuscated" | "obf" => 'k',
        "reset" => 'r',
        _ => return None,
    })
}

fn tag_code(tag: &str) -> Option<char> {
    let tag = tag.trim().to_ascii_lowercase();
    let tag = tag.strip_prefix("color:").or_else(|| tag.strip_prefix("c:")).unwrap_or(&tag);
    let tag = if tag == "grey" { "gray" } else if tag == "dark_grey" { "dark_gray" } else { tag };
    if let Some(hex) = tag.strip_prefix('#') {
        return u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6).map(nearest_color);
    }
    COLORS.iter().find(|c| c.1 == tag).map(|c| c.0).or_else(|| decoration(tag))
}

/// A format of DiscordSRV's messages as `§` codes: the MiniMessage tags its defaults use
/// (colours by name or `#rrggbb`, decorations, `<reset>`; a closing tag resets) and `&` codes.
/// Unknown tags stay as they are.
pub fn to_legacy(format: &str) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let tag = &rest[1..end];
            let code = match tag.strip_prefix('/') {
                Some(closed) if tag_code(closed).is_some() => Some('r'),
                Some(_) => None,
                None => tag_code(tag),
            };
            if let Some(code) = code {
                out.push('§');
                out.push(code);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    translate_ampersand(&out)
}

/// `&x` codes (`0-9a-fk-or`) as `§x`.
pub fn translate_ampersand(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&'
            && let Some(&n) = chars.peek()
            && is_code(n)
        {
            out.push('§');
            out.push(n.to_ascii_lowercase());
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

fn is_code(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r')
}

/// Minecraft formatting codes (`§l`/`&l` and the rest) as Discord markdown, as MCDiscordReserializer's
/// `DiscordSerializer` writes them: bold `**`, italic `*`, underline `__`, strikethrough `~~` around the
/// text they cover. Colour codes (and `r`) end every open format, as in the game; obfuscation is dropped.
/// A format's closing marker goes before the spaces that end the text, which Discord needs.
pub fn legacy_to_markdown(text: &str) -> String {
    const MARKERS: [(char, &str); 4] = [('l', "**"), ('o', "*"), ('n', "__"), ('m', "~~")];
    let mut out = String::with_capacity(text.len() + 8);
    let mut open: Vec<&'static str> = Vec::new();
    fn close_all(out: &mut String, open: &mut Vec<&'static str>) {
        while let Some(marker) = open.pop() {
            let kept = out.trim_end_matches(' ').len();
            let spaces = out.split_off(kept);
            out.push_str(marker);
            out.push_str(&spaces);
        }
    }
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let coded = c == '§' || (c == '&' && chars.peek().is_some_and(|n| is_code(*n)));
        if !coded {
            out.push(c);
            continue;
        }
        let Some(code) = chars.next().map(|d| d.to_ascii_lowercase()) else { break };
        if let Some((_, marker)) = MARKERS.iter().find(|(k, _)| *k == code) {
            if !open.contains(marker) {
                out.push_str(marker);
                open.push(marker);
            }
        } else if code == 'k' {
            // Obfuscated text has no Discord form.
        } else {
            close_all(&mut out, &mut open);
        }
    }
    close_all(&mut out, &mut open);
    out
}

/// Discord markdown as Minecraft `§` codes (MCDiscordReserializer's `MinecraftSerializer`, for the four
/// formats `Experiment_MCDiscordReserializer_ToMinecraft` names): `**` bold, `*` or `_` italic, `__`
/// underline, `~~` strikethrough. A marker with no closing partner stays as written, a `_` inside a word
/// is no marker, and `\*` is an escaped one. Closing a format resets and puts back the ones still open.
pub fn markdown_to_legacy(text: &str) -> String {
    const CODES: [(&'static str, char); 5] = [("**", 'l'), ("__", 'n'), ("~~", 'm'), ("*", 'o'), ("_", 'o')];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut open: Vec<(&'static str, char)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1).is_some_and(|n| matches!(n, '*' | '_' | '~' | '\\')) {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        let rest: String = chars[i..].iter().collect();
        let Some((marker, code)) = CODES.iter().copied().find(|(m, _)| rest.starts_with(m)) else {
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let after = &rest[marker.len()..];
        let word_char = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        if let Some(at) = open.iter().position(|(m, _)| *m == marker) {
            // Closing: an `_` closes only at a word end.
            if marker == "_" && word_char(after.chars().next()) {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            open.remove(at);
            out.push_str("§r");
            for (_, c) in &open {
                out.push('§');
                out.push(*c);
            }
        } else {
            // Opening: needs a closing partner later; an `_` opens only at a word start.
            let boundary = marker != "_" || !word_char(i.checked_sub(1).and_then(|p| chars.get(p)).copied());
            let partner = {
                let mut from = 0;
                let mut found = false;
                while let Some(p) = after[from..].find(marker) {
                    let tail = &after[from + p + marker.len()..];
                    if marker != "_" || !word_char(tail.chars().next()) {
                        found = true;
                        break;
                    }
                    from += p + marker.len();
                }
                found
            };
            if !boundary || !partner || after.starts_with(' ') {
                out.push_str(marker);
                i += marker.len();
                continue;
            }
            out.push('§');
            out.push(code);
            open.push((marker, code));
        }
        i += marker.chars().count();
    }
    out
}

/// `§x` and `&x` codes removed (text from Discord whose author may not use colours).
pub fn strip_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
            continue;
        }
        if c == '&'
            && let Some(&n) = chars.peek()
            && is_code(n)
        {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Discord markdown characters escaped, so a name like `jeb_` shows as written.
pub fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '*' | '_' | '~' | '|' | '`' | '>' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Every `%key%` of `pairs` replaced, in one pass (a value is never read as a placeholder).
pub fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    'outer: while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        for (key, value) in pairs {
            if let Some(tail) = after.strip_prefix(key).and_then(|t| t.strip_prefix('%')) {
                out.push_str(value);
                rest = tail;
                continue 'outer;
            }
        }
        out.push('%');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// At most `max` characters (not bytes).
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 || text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

#[cfg(test)]
mod reserializer_tests {
    use super::*;

    #[test]
    fn legacy_codes_become_markdown() {
        assert_eq!(legacy_to_markdown("&lbold &r&aplain"), "**bold** plain");
        // Both stay on until a colour or reset: underline is still open when the strike starts.
        assert_eq!(legacy_to_markdown("§l§obold italic§r §nunder §mstrike"), "***bold italic*** __under ~~strike~~__");
        assert_eq!(legacy_to_markdown("&kgone &cred"), "gone red");
        assert_eq!(legacy_to_markdown("no codes & such"), "no codes & such");
    }

    #[test]
    fn markdown_becomes_legacy_codes() {
        assert_eq!(markdown_to_legacy("a **b** c"), "a §lb§r c");
        assert_eq!(markdown_to_legacy("*i* __u__ ~~s~~ _j_"), "§oi§r §nu§r §ms§r §oj§r");
        assert_eq!(markdown_to_legacy("**a *b* c**"), "§la §ob§r§l c§r");
    }

    #[test]
    fn stray_and_escaped_markers_stay() {
        assert_eq!(markdown_to_legacy("2 * 3 = 6"), "2 * 3 = 6");
        assert_eq!(markdown_to_legacy("snake_case_name"), "snake_case_name");
        assert_eq!(markdown_to_legacy("\\*not italic\\*"), "*not italic*");
        assert_eq!(markdown_to_legacy("**open only"), "**open only");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_discord_format_becomes_section_codes() {
        let f = to_legacy("[<aqua>Discord</aqua> | &4Owner<reset>] %name% » %message%");
        assert_eq!(f, "[§bDiscord§r | §4Owner§r] %name% » %message%");
        assert_eq!(to_legacy("<#ff5555>x</#ff5555> <b>y</b>"), "§cx§r §ly§r");
        assert_eq!(to_legacy("a <notatag> b < c"), "a <notatag> b < c");
        assert_eq!(to_legacy("<color:gold>g"), "§6g");
    }

    #[test]
    fn colours_are_approximated() {
        assert_eq!(nearest_color(0xE74C3C), 'c');
        assert_eq!(nearest_color(0x3498DB), '3');
        assert_eq!(nearest_color(0x000000), '0');
        assert_eq!(nearest_color(0xF1C40F), '6');
    }

    #[test]
    fn codes_are_stripped_and_translated() {
        assert_eq!(strip_codes("&chi §lthere & you"), "hi there & you");
        assert_eq!(translate_ampersand("&cRed &zno"), "§cRed &zno");
    }

    #[test]
    fn markdown_is_escaped() {
        assert_eq!(escape_markdown("jeb_ *x*"), "jeb\\_ \\*x\\*");
    }

    #[test]
    fn placeholders_fill_once() {
        let s = fill("%name%: %message% (100%)", &[("name", "%message%"), ("message", "hi")]);
        assert_eq!(s, "%message%: hi (100%)");
        assert_eq!(truncate("abcdef", 3), "abc");
        assert_eq!(truncate("abc", 0), "abc");
    }
}
