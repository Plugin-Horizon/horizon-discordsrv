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
