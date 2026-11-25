//! `alerts.yml`: messages to Discord when something happens in the game or a command is run
//! (DiscordSRV's `AlertListener`). Each alert has a `Trigger` (an event name or `/command`), optional
//! `Conditions`, a `Channel`, and a message (`Content`, `Embed`, `Webhook`) written as in
//! messages.yml, with `{placeholders}` and `${expressions}`.
//!
//! What maps onto Horizon: the triggers `PlayerJoinEvent`, `PlayerQuitEvent`, `PlayerDeathEvent`,
//! `PlayerAdvancementDoneEvent`, `AsyncPlayerChatEvent` / `AsyncChatEvent`,
//! `PlayerCommandPreprocessEvent`, `ServerCommandEvent` (simple or fully qualified names) and every
//! `/command`. Any other Bukkit or plugin event class does not exist here, so such an alert is
//! reported once at start and never fires. DiscordSRV evaluates conditions and `${...}` with Spring's
//! expression language over live Java objects; this module reads the expression subset below over the
//! facts of the event: literals, `#player` (name, displayName, uniqueId, world.name, gameMode, ping,
//! `hasPermission(..)`), `#args`, `#allArgs`, `#command`, `#server.onlinePlayers`, the event's
//! `message`, the comparison and boolean operators, and the usual string and list methods. An
//! expression outside it is an error, which (as in DiscordSRV) is logged and leaves a condition
//! true and a `${...}` as written.

use serde_json::Value;

use crate::discordsrv_bridge::config::MessageTemplate;

/// What happened, as far as alerts know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Join,
    Quit,
    Death,
    Advancement,
    Chat,
    PlayerCommand,
    ConsoleCommand,
}

impl Kind {
    /// The simple event class names that mean this kind.
    fn names(self) -> &'static [&'static str] {
        match self {
            Kind::Join => &["PlayerJoinEvent"],
            Kind::Quit => &["PlayerQuitEvent"],
            Kind::Death => &["PlayerDeathEvent"],
            Kind::Advancement => &["PlayerAdvancementDoneEvent"],
            Kind::Chat => &["AsyncPlayerChatEvent", "AsyncChatEvent"],
            Kind::PlayerCommand => &["PlayerCommandPreprocessEvent"],
            Kind::ConsoleCommand => &["ServerCommandEvent"],
        }
    }

    /// Whether a trigger text names an event this port can raise at all.
    pub fn known_event(trigger: &str) -> bool {
        let simple = trigger.rsplit('.').next().unwrap_or(trigger);
        [Kind::Join, Kind::Quit, Kind::Death, Kind::Advancement, Kind::Chat, Kind::PlayerCommand, Kind::ConsoleCommand]
            .iter()
            .any(|k| k.names().iter().any(|n| n.eq_ignore_ascii_case(simple)))
    }
}

/// One `Alerts` entry.
#[derive(Debug, Clone)]
pub struct Alert {
    pub triggers: Vec<String>,
    pub channels: Vec<String>,
    pub conditions: Vec<String>,
    pub ignore_cancelled: bool,
    pub template: MessageTemplate,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::Number(n)) => vec![n.to_string()],
        _ => Vec::new(),
    }
}

/// Reads the `Alerts` list. A message section left without `Enabled` is on, as DiscordSRV has it for
/// alerts (`Embed: Enabled: true` is their default); a bad entry is skipped with a warning.
pub fn parse(raw: &[Value], warnings: &mut Vec<String>) -> Vec<Alert> {
    let mut out = Vec::new();
    for (i, v) in raw.iter().enumerate() {
        let Some(map) = v.as_object() else {
            warnings.push(format!("Alert #{}: not a mapping, skipped", i + 1));
            continue;
        };
        let triggers = strings(map.get("Trigger"));
        if triggers.is_empty() {
            warnings.push(format!("Alert #{}: no Trigger, skipped", i + 1));
            continue;
        }
        for t in &triggers {
            if !t.starts_with('/') && !Kind::known_event(t) {
                warnings.push(format!(
                    "Alert #{}: the trigger \"{t}\" is not an event this server has (known: player join, quit, death, advancement, chat, command, console command, and any /command)",
                    i + 1
                ));
            }
        }
        let mut template_json = map.clone();
        template_json.insert("Enabled".into(), Value::Bool(true));
        if let Some(Value::Object(embed)) = template_json.get_mut("Embed") {
            embed.entry("Enabled").or_insert(Value::Bool(true));
            // `Color` takes a hex code or an RGB integer; the template reads text.
            if let Some(color) = embed.get_mut("Color") {
                if let Value::Number(n) = color {
                    *color = Value::String(n.to_string());
                }
            }
        }
        for key in ["Trigger", "Conditions", "Channel", "Async", "IgnoreCancelled"] {
            template_json.remove(key);
        }
        let template = match serde_json::from_value::<MessageTemplate>(Value::Object(template_json)) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("Alert #{}: {e}, skipped", i + 1));
                continue;
            }
        };
        out.push(Alert {
            triggers,
            channels: strings(map.get("Channel")),
            conditions: strings(map.get("Conditions")),
            ignore_cancelled: map.get("IgnoreCancelled").and_then(Value::as_bool).unwrap_or(true),
            template,
        });
    }
    out
}

/// The player an event is about.
#[derive(Debug, Clone, Default)]
pub struct PlayerFacts {
    pub name: String,
    pub uuid: String,
    pub world: String,
    pub game_mode: String,
    pub ping: i64,
}

/// What an expression can see.
pub struct Event<'a> {
    pub kind: Kind,
    pub player: Option<&'a PlayerFacts>,
    /// The chat message, the death message, the advancement title, or the command line (no slash).
    pub message: &'a str,
    pub cancelled: bool,
    pub online: Vec<String>,
    pub has_permission: &'a dyn Fn(&str) -> bool,
}

impl Event<'_> {
    /// The command line without the slash, the namespace taken off its first word.
    fn command(&self) -> Option<String> {
        if !matches!(self.kind, Kind::PlayerCommand | Kind::ConsoleCommand) {
            return None;
        }
        let line = self.message.trim_start_matches('/').trim();
        if line.is_empty() {
            return None;
        }
        let mut parts = line.splitn(2, ' ');
        let base = parts.next().unwrap_or("");
        let base = base.rsplit(':').next().unwrap_or(base);
        Some(match parts.next() {
            Some(rest) => format!("{base} {rest}"),
            None => base.to_string(),
        })
    }

    fn args(&self) -> Vec<String> {
        self.command()
            .and_then(|c| c.split_once(' ').map(|(_, rest)| rest.split(' ').map(str::to_string).collect()))
            .unwrap_or_default()
    }
}

/// Does the alert fire for this event? A trigger matches a `/command` by its first word, an event by name.
pub fn triggered(alert: &Alert, ev: &Event) -> bool {
    alert.triggers.iter().any(|t| {
        if let Some(cmd) = t.strip_prefix('/') {
            return ev.command().is_some_and(|c| c.split_whitespace().next().unwrap_or("").eq_ignore_ascii_case(cmd));
        }
        let simple = t.rsplit('.').next().unwrap_or(t);
        ev.kind.names().iter().any(|n| n.eq_ignore_ascii_case(simple))
    })
}

/// Checks the conditions and writes the `${...}` expressions of the message in; None when the alert
/// does not apply (cancelled event, a condition that is false).
pub fn prepare(alert: &Alert, ev: &Event, log: &mut Vec<String>) -> Option<MessageTemplate> {
    if ev.cancelled && alert.ignore_cancelled {
        return None;
    }
    for c in &alert.conditions {
        match eval(c, ev) {
            Ok(Val::Bool(false)) => return None,
            Ok(_) => {}
            Err(e) => log.push(format!("Error while evaluating expression \"{c}\" -> {e}")),
        }
    }
    let f = |s: &str| expand(s, ev, log);
    Some(map_strings(&alert.template, &f))
}

/// The template with `f` applied to every text field.
pub fn map_strings(t: &MessageTemplate, f: &dyn Fn(&str) -> String) -> MessageTemplate {
    let mut o = t.clone();
    o.content = f(&o.content);
    o.webhook.name = f(&o.webhook.name);
    o.webhook.avatar_url = f(&o.webhook.avatar_url);
    let e = &mut o.embed;
    e.color = f(&e.color);
    e.author.image_url = f(&e.author.image_url);
    e.author.name = f(&e.author.name);
    e.author.url = f(&e.author.url);
    e.thumbnail_url = f(&e.thumbnail_url);
    e.title.text = f(&e.title.text);
    e.title.url = f(&e.title.url);
    e.description = f(&e.description);
    e.fields = e.fields.iter().map(|x| f(x)).collect();
    e.image_url = f(&e.image_url);
    e.footer.text = f(&e.footer.text);
    e.footer.icon_url = f(&e.footer.icon_url);
    o
}

/// Every `{key}` of `pairs` replaced, in one pass (a value is never read as a placeholder); an unknown
/// `{...}` stays as it is.
pub fn fill_braces(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        for (key, value) in pairs {
            if let Some(tail) = after.strip_prefix(key).and_then(|t| t.strip_prefix('}')) {
                out.push_str(value);
                rest = tail;
                continue 'outer;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Every `${expression}` evaluated; one that fails stays as written.
pub fn expand(text: &str, ev: &Event, log: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = closing_brace(after) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = &after[..end];
        match eval(expr, ev) {
            Ok(v) => out.push_str(&v.text()),
            Err(e) => {
                log.push(format!("Error while evaluating expression \"{expr}\" -> {e}"));
                out.push_str(&rest[start..start + 2 + end + 1]);
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The byte index of the `}` that closes an expression (quotes and nested braces respected).
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' => depth += 1,
                '}' if depth == 0 => return Some(i),
                '}' => depth -= 1,
                _ => {}
            },
        }
    }
    None
}

// ---- the expression language -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<String>),
}

impl Val {
    fn text(&self) -> String {
        match self {
            Val::Null => "null".into(),
            Val::Bool(b) => b.to_string(),
            Val::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Val::Num(n) => n.to_string(),
            Val::Str(s) => s.clone(),
            Val::List(l) => format!("[{}]", l.join(", ")),
        }
    }

    fn truthy(&self) -> Result<bool, String> {
        match self {
            Val::Bool(b) => Ok(*b),
            other => Err(format!("{other:?} is not a boolean")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Hash,
    Dot,
    LParen,
    RParen,
    Comma,
    Op(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' => i += 1,
            '#' => {
                out.push(Tok::Hash);
                i += 1;
            }
            '.' if !chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                out.push(Tok::Dot);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '\'' | '"' => {
                let q = c;
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(&d) = chars.get(i) else { return Err("unterminated string".into()) };
                    i += 1;
                    if d == q {
                        // A doubled quote is one quote, as in SpEL.
                        if chars.get(i) == Some(&q) {
                            s.push(q);
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    s.push(d);
                }
                out.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let t: String = chars[start..i].iter().collect();
                out.push(Tok::Num(t.parse().map_err(|_| format!("bad number {t}"))?));
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let op = match two.as_str() {
                    "==" => Some("=="),
                    "!=" => Some("!="),
                    "<=" => Some("<="),
                    ">=" => Some(">="),
                    "&&" => Some("&&"),
                    "||" => Some("||"),
                    _ => None,
                };
                if let Some(op) = op {
                    out.push(Tok::Op(op));
                    i += 2;
                } else {
                    let one = match c {
                        '<' => "<",
                        '>' => ">",
                        '!' => "!",
                        other => return Err(format!("unexpected character {other}")),
                    };
                    out.push(Tok::Op(one));
                    i += 1;
                }
            }
        }
    }
    Ok(out)
}

/// What a property chain is standing on.
#[derive(Debug, Clone)]
enum Obj {
    Event,
    Player,
    World(String),
    Server,
    V(Val),
}

struct Parser<'a, 'b> {
    toks: Vec<Tok>,
    at: usize,
    ev: &'a Event<'b>,
}

pub fn eval(src: &str, ev: &Event) -> Result<Val, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, at: 0, ev };
    let v = p.or()?;
    if p.at != p.toks.len() {
        return Err("unexpected text after the expression".into());
    }
    Ok(v)
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Val, String> {
        let mut left = self.and()?;
        while self.op("||") {
            let right = self.and()?;
            left = Val::Bool(left.truthy()? || right.truthy()?);
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Val, String> {
        let mut left = self.eq()?;
        while self.op("&&") {
            let right = self.eq()?;
            left = Val::Bool(left.truthy()? && right.truthy()?);
        }
        Ok(left)
    }

    fn eq(&mut self) -> Result<Val, String> {
        let mut left = self.rel()?;
        loop {
            let equal = if self.op("==") {
                true
            } else if self.op("!=") {
                false
            } else {
                return Ok(left);
            };
            let right = self.rel()?;
            left = Val::Bool((left == right) == equal);
        }
    }

    fn rel(&mut self) -> Result<Val, String> {
        let left = self.unary()?;
        for op in ["<=", ">=", "<", ">"] {
            if self.op(op) {
                let right = self.unary()?;
                let (Val::Num(a), Val::Num(b)) = (&left, &right) else {
                    return Err(format!("{op} needs numbers"));
                };
                return Ok(Val::Bool(match op {
                    "<=" => a <= b,
                    ">=" => a >= b,
                    "<" => a < b,
                    _ => a > b,
                }));
            }
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Val, String> {
        if self.op("!") {
            return Ok(Val::Bool(!self.unary()?.truthy()?));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Val, String> {
        match self.toks.get(self.at).cloned() {
            Some(Tok::Num(n)) => {
                self.at += 1;
                Ok(Val::Num(n))
            }
            Some(Tok::Str(s)) => {
                self.at += 1;
                Ok(Val::Str(s))
            }
            Some(Tok::LParen) => {
                self.at += 1;
                let v = self.or()?;
                if self.peek() != Some(&Tok::RParen) {
                    return Err("missing )".into());
                }
                self.at += 1;
                Ok(v)
            }
            Some(Tok::Ident(w)) if w == "true" || w == "false" => {
                self.at += 1;
                Ok(Val::Bool(w == "true"))
            }
            Some(Tok::Ident(w)) if w == "null" => {
                self.at += 1;
                Ok(Val::Null)
            }
            Some(Tok::Hash) => {
                self.at += 1;
                let Some(Tok::Ident(root)) = self.toks.get(self.at).cloned() else {
                    return Err("a name must follow #".into());
                };
                self.at += 1;
                let start = match root.as_str() {
                    "player" | "sender" => Obj::Player,
                    "server" => Obj::Server,
                    "event" => Obj::Event,
                    "args" => Obj::V(Val::List(self.ev.args())),
                    "allArgs" => Obj::V(Val::Str(self.ev.args().join(" "))),
                    "command" => Obj::V(self.ev.command().map_or(Val::Null, Val::Str)),
                    other => return Err(format!("unknown variable #{other}")),
                };
                self.chain(start)
            }
            // A bare name is a property of the event, as in DiscordSRV's evaluation context.
            Some(Tok::Ident(_)) => self.chain(Obj::Event),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    /// `.name`, `.name(args)`, or (for a bare start) the first name itself as a property.
    fn chain(&mut self, start: Obj) -> Result<Val, String> {
        let mut cur = start;
        let mut first = matches!(cur, Obj::Event) && matches!(self.peek(), Some(Tok::Ident(_)));
        loop {
            let name = if first {
                first = false;
                match self.toks.get(self.at).cloned() {
                    Some(Tok::Ident(n)) => {
                        self.at += 1;
                        n
                    }
                    _ => break,
                }
            } else if self.peek() == Some(&Tok::Dot) {
                self.at += 1;
                match self.toks.get(self.at).cloned() {
                    Some(Tok::Ident(n)) => {
                        self.at += 1;
                        n
                    }
                    _ => return Err("a name must follow .".into()),
                }
            } else {
                break;
            };
            let mut args = Vec::new();
            let call = self.peek() == Some(&Tok::LParen);
            if call {
                self.at += 1;
                if self.peek() != Some(&Tok::RParen) {
                    loop {
                        args.push(self.or()?);
                        if self.peek() == Some(&Tok::Comma) {
                            self.at += 1;
                            continue;
                        }
                        break;
                    }
                }
                if self.peek() != Some(&Tok::RParen) {
                    return Err("missing )".into());
                }
                self.at += 1;
            }
            cur = self.step(cur, &name, call, &args)?;
        }
        Ok(match cur {
            Obj::V(v) => v,
            Obj::Player => Val::Str(self.ev.player.map(|p| p.name.clone()).unwrap_or_default()),
            Obj::World(w) => Val::Str(w),
            Obj::Server | Obj::Event => Val::Null,
        })
    }

    fn step(&self, cur: Obj, name: &str, call: bool, args: &[Val]) -> Result<Obj, String> {
        let s = |v: &str| -> Result<Obj, String> { Ok(Obj::V(Val::Str(v.to_string()))) };
        match cur {
            Obj::Event => match name {
                "message" | "getMessage" | "deathMessage" | "getDeathMessage" | "joinMessage" | "quitMessage" | "title" => {
                    s(self.ev.message)
                }
                "player" | "getPlayer" => Ok(Obj::Player),
                "cancelled" | "isCancelled" => Ok(Obj::V(Val::Bool(self.ev.cancelled))),
                "command" => Ok(Obj::V(self.ev.command().map_or(Val::Null, Val::Str))),
                "args" => Ok(Obj::V(Val::List(self.ev.args()))),
                other => Err(format!("the event has no property {other}")),
            },
            Obj::Player => {
                let p = self.ev.player.ok_or("this event has no player")?;
                match name {
                    "name" | "getName" | "displayName" | "getDisplayName" => s(&p.name),
                    "uniqueId" | "getUniqueId" => s(&p.uuid),
                    "world" | "getWorld" => Ok(Obj::World(p.world.clone())),
                    "gameMode" | "getGameMode" => s(&p.game_mode.to_uppercase()),
                    "ping" | "getPing" => Ok(Obj::V(Val::Num(p.ping as f64))),
                    "hasPermission" | "isPermissionSet" => {
                        let Some(Val::Str(node)) = args.first() else { return Err("hasPermission needs a node".into()) };
                        Ok(Obj::V(Val::Bool((self.ev.has_permission)(node))))
                    }
                    other => Err(format!("a player has no property {other}")),
                }
            }
            Obj::World(w) => match name {
                "name" | "getName" => s(&w),
                other => Err(format!("a world has no property {other}")),
            },
            Obj::Server => match name {
                "onlinePlayers" | "getOnlinePlayers" => Ok(Obj::V(Val::List(self.ev.online.clone()))),
                other => Err(format!("the server has no property {other}")),
            },
            Obj::V(v) => method(&v, name, call, args).map(Obj::V),
        }
    }
}

/// Methods and properties of a value.
fn method(v: &Val, name: &str, call: bool, args: &[Val]) -> Result<Val, String> {
    let text = |a: Option<&Val>| a.map(Val::text).unwrap_or_default();
    match (v, name) {
        (Val::List(l), "size" | "length") => Ok(Val::Num(l.len() as f64)),
        (Val::List(l), "isEmpty" | "empty") => Ok(Val::Bool(l.is_empty())),
        (Val::List(l), "get") => {
            let Some(Val::Num(i)) = args.first() else { return Err("get needs an index".into()) };
            l.get(*i as usize).cloned().map(Val::Str).ok_or_else(|| format!("index {} out of bounds for length {}", *i as i64, l.len()))
        }
        (Val::List(l), "contains") => Ok(Val::Bool(l.iter().any(|x| *x == text(args.first())))),
        (Val::Str(s), "toLowerCase") => Ok(Val::Str(s.to_lowercase())),
        (Val::Str(s), "toUpperCase") => Ok(Val::Str(s.to_uppercase())),
        (Val::Str(s), "trim") => Ok(Val::Str(s.trim().to_string())),
        (Val::Str(s), "length") => Ok(Val::Num(s.chars().count() as f64)),
        (Val::Str(s), "isEmpty" | "empty") => Ok(Val::Bool(s.is_empty())),
        (Val::Str(s), "contains") => Ok(Val::Bool(s.contains(&text(args.first())))),
        (Val::Str(s), "startsWith") => Ok(Val::Bool(s.starts_with(&text(args.first())))),
        (Val::Str(s), "endsWith") => Ok(Val::Bool(s.ends_with(&text(args.first())))),
        (Val::Str(s), "equals") => Ok(Val::Bool(*s == text(args.first()))),
        (Val::Str(s), "equalsIgnoreCase") => Ok(Val::Bool(s.eq_ignore_ascii_case(&text(args.first())))),
        (other, "name" | "toString") if call => Ok(Val::Str(other.text())),
        (other, "name") => Ok(Val::Str(other.text())),
        (other, member) => Err(format!("no method or property {member} on {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn player() -> PlayerFacts {
        PlayerFacts { name: "Steve".into(), uuid: "00000000-0000-0000-0000-000000000001".into(), world: "world_nether".into(), game_mode: "creative".into(), ping: 42 }
    }

    fn event<'a>(kind: Kind, message: &'a str, p: &'a PlayerFacts, perm: &'a dyn Fn(&str) -> bool) -> Event<'a> {
        Event { kind, player: Some(p), message, cancelled: false, online: vec!["Steve".into(), "Alex".into()], has_permission: perm }
    }

    #[test]
    fn alerts_are_read_with_defaults() {
        let raw = vec![
            json!({"Trigger": ["PlayerJoinEvent", "/gamemode"], "Channel": "main", "Conditions": ["#player.world.name == 'x'"],
                   "Embed": {"Color": "#00ff00", "Author": {"Name": "{username} did... something."}}}),
            json!({"Trigger": "org.example.SomeEvent", "Channel": ["a", "b"], "IgnoreCancelled": false}),
            json!({"Channel": "x"}),
            json!("nonsense"),
        ];
        let mut w = Vec::new();
        let alerts = parse(&raw, &mut w);
        assert_eq!(alerts.len(), 2);
        assert!(alerts[0].template.embed.enabled, "Embed without Enabled is on");
        assert_eq!(alerts[0].template.embed.author.name, "{username} did... something.");
        assert_eq!(alerts[1].channels, vec!["a", "b"]);
        assert!(!alerts[1].ignore_cancelled && alerts[0].ignore_cancelled);
        assert!(w.iter().any(|m| m.contains("org.example.SomeEvent") && m.contains("not an event")), "{w:?}");
        assert!(w.iter().any(|m| m.contains("no Trigger")));
        assert!(w.iter().any(|m| m.contains("not a mapping")));
    }

    #[test]
    fn triggers_match_events_and_commands() {
        let p = player();
        let no = |_: &str| false;
        let alert = |t: Value| parse(&[json!({"Trigger": t, "Channel": "c"})], &mut Vec::new()).remove(0);
        let join = event(Kind::Join, "", &p, &no);
        assert!(triggered(&alert(json!("PlayerJoinEvent")), &join));
        assert!(triggered(&alert(json!("org.bukkit.event.player.PlayerJoinEvent")), &join));
        assert!(!triggered(&alert(json!("PlayerQuitEvent")), &join));
        let cmd = event(Kind::PlayerCommand, "minecraft:gamemode creative Steve", &p, &no);
        assert!(triggered(&alert(json!("/gamemode")), &cmd));
        assert!(triggered(&alert(json!("PlayerCommandPreprocessEvent")), &cmd));
        assert!(!triggered(&alert(json!("/give")), &cmd));
        assert!(triggered(&alert(json!(["/x", "AsyncChatEvent"])), &event(Kind::Chat, "hi", &p, &no)));
    }

    #[test]
    fn conditions_read_the_event() {
        let p = player();
        let yes = |n: &str| n == "minecraft.command.gamemode";
        let e = event(Kind::PlayerCommand, "gamemode creative Steve", &p, &yes);
        for (expr, want) in [
            ("#player.hasPermission('minecraft.command.gamemode') == true", true),
            ("#player.hasPermission('other')", false),
            ("#args.size() == 2", true),
            ("#args.get(0) == 'creative'", true),
            ("#allArgs == 'creative Steve'", true),
            ("#command.startsWith('gamemode')", true),
            ("#player.world.name == 'world_the_end'", false),
            ("#player.world.name != 'world_the_end' && #player.ping >= 40", true),
            ("#player.gameMode.name() == 'CREATIVE'", true),
            ("!(#server.onlinePlayers.size() > 5) || false", true),
            ("#player.name.toLowerCase() == 'steve'", true),
        ] {
            assert_eq!(eval(expr, &e), Ok(Val::Bool(want)), "{expr}");
        }
        assert!(eval("#player.nonsense", &e).is_err());
        assert!(eval("#unknown", &e).is_err());
        assert!(eval("1 +", &e).is_err());
    }

    #[test]
    fn bare_names_are_event_properties() {
        let p = player();
        let no = |_: &str| false;
        let chat = event(Kind::Chat, "hello there", &p, &no);
        assert_eq!(eval("message.contains('hello')", &chat), Ok(Val::Bool(true)));
        assert_eq!(eval("player.name", &chat), Ok(Val::Str("Steve".into())));
        assert_eq!(eval("message == 'x'", &chat), Ok(Val::Bool(false)));
    }

    #[test]
    fn prepare_checks_conditions_and_fills_expressions() {
        let p = player();
        let no = |_: &str| false;
        let alert = parse(
            &[json!({"Trigger": "/me", "Channel": "me", "Conditions": ["#args.size() > 0", "#oops"],
                     "Embed": {"Author": {"Name": "{username} says ${#allArgs} (${#args.size()} words) ${bad(}"}}})],
            &mut Vec::new(),
        )
        .remove(0);
        let mut log = Vec::new();
        let cmd = event(Kind::PlayerCommand, "me waves hello", &p, &no);
        let t = prepare(&alert, &cmd, &mut log).expect("applies");
        // The broken condition is logged and ignored; the broken expression stays as written.
        assert_eq!(t.embed.author.name, "{username} says waves hello (2 words) ${bad(}");
        assert_eq!(log.len(), 2, "{log:?}");
        // No arguments: the first condition is false.
        let bare = event(Kind::PlayerCommand, "me", &p, &no);
        assert!(prepare(&alert, &bare, &mut Vec::new()).is_none());
    }

    #[test]
    fn cancelled_events_are_skipped_unless_asked() {
        let p = player();
        let no = |_: &str| false;
        let mut e = event(Kind::Chat, "x", &p, &no);
        e.cancelled = true;
        let a = parse(&[json!({"Trigger": "AsyncChatEvent", "Channel": "c"})], &mut Vec::new()).remove(0);
        assert!(prepare(&a, &e, &mut Vec::new()).is_none());
        let b = parse(&[json!({"Trigger": "AsyncChatEvent", "Channel": "c", "IgnoreCancelled": false})], &mut Vec::new()).remove(0);
        assert!(prepare(&b, &e, &mut Vec::new()).is_some());
    }

    #[test]
    fn braces_are_filled_in_one_pass() {
        let pairs = [("name", "{tps}"), ("tps", "19.99")];
        assert_eq!(fill_braces("{name} runs at {tps} {other} {", &pairs), "{tps} runs at 19.99 {other} {");
    }

    #[test]
    fn expression_text_forms() {
        let p = player();
        let no = |_: &str| false;
        let e = event(Kind::Join, "", &p, &no);
        let mut log = Vec::new();
        assert_eq!(expand("a ${1} b ${'x' == 'x'} c ${#player.world.name} d ${'it''s'}", &e, &mut log), "a 1 b true c world_nether d it's");
        assert_eq!(expand("open ${ nothing", &e, &mut log), "open ${ nothing");
        assert!(log.is_empty());
    }
}
