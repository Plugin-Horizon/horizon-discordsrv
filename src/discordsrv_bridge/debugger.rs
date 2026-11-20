//! `/discord debugger`: DiscordSRV's toggleable debug categories (`Debug` enum, `CommandDebugger`).
//! The categories switch `[<CATEGORY> DEBUG]` log lines on ([`log`]); `upload` writes the report to a
//! file in the mod's config folder instead of uploading it to a paste service (no server data
//! leaves the machine), and says where the file is.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// The categories DiscordSRV's `Debug` enum has, with their aliases.
const CATEGORIES: [(&str, &[&str]); 17] = [
    ("MINECRAFT_TO_DISCORD", &["minecrafttodiscord", "mctodiscord", "todiscord", "minecraft", "minecraftchat"]),
    ("DISCORD_TO_MINECRAFT", &["discordtominecraft", "discordtomc", "tominecraft", "discord", "discordchat"]),
    ("GROUP_SYNC", &["group", "groups", "gsync", "role", "roles", "groupsync", "rolesync"]),
    ("PRESENCE", &["game", "gamestatus", "playing", "playingstatus", "status"]),
    ("VOICE", &["voicemodule"]),
    ("REQUIRE_LINK", &["requirelink", "requirelink2play", "requirelinktoplay", "link2play", "linktoplay"]),
    ("NICKNAME_SYNC", &["nickname", "nicknamesync"]),
    ("ALERTS", &["alert"]),
    ("WATCHDOG", &["watchdog"]),
    ("BAN_SYNCHRONIZATION", &["ban", "bans"]),
    ("LP_CONTEXTS", &["luckpermscontexts", "luckpermscontext", "lpcontexts", "lpcontext"]),
    ("ACCOUNT_LINKING", &["linkedaccounts", "accountlinking"]),
    ("CHANNEL_UPDATER", &["channelupdater", "statschannels"]),
    ("UNCATEGORIZED", &["all"]),
    ("JDA", &[]),
    ("JDA_REST_ACTIONS", &["jdarestactions", "jdarest", "restactions", "rest"]),
    (
        "CALLSTACKS",
        &["stack", "stacks", "callstack", "trace", "traces", "stacktrace", "errors", "exceptions", "exception", "except"],
    ),
];

/// The categories `/discord debugger start` switched on (DiscordSRV's `getDebuggerCategories`).
static ACTIVE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// `Debug.matches`: whether `arg` names the category `name` (its name, an alias, `all`, `chat`).
fn matches(name: &str, aliases: &[&str], arg: &str) -> bool {
    let all_ok = !matches!(name, "CALLSTACKS" | "JDA" | "JDA_REST_ACTIONS");
    (arg.eq_ignore_ascii_case("all") && all_ok)
        || (arg.eq_ignore_ascii_case("chat") && matches!(name, "MINECRAFT_TO_DISCORD" | "DISCORD_TO_MINECRAFT"))
        || arg.eq_ignore_ascii_case(name)
        || aliases.iter().any(|a| arg.eq_ignore_ascii_case(a))
}

/// Whether `arg` is a valid category word.
fn valid(arg: &str) -> bool {
    CATEGORIES.iter().any(|(name, aliases)| matches(name, aliases, arg))
}

/// `Debug.isVisible`: whether category `name` logs, by the switched-on words or the config's `Debug` list.
pub fn visible(name: &str, config_list: &[String]) -> bool {
    let Some((_, aliases)) = CATEGORIES.iter().find(|(n, _)| *n == name) else { return false };
    let active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    active.iter().any(|a| matches(name, aliases, a)) || config_list.iter().any(|a| matches(name, aliases, a))
}

/// A `[<CATEGORY> DEBUG] message` line when the category is visible.
pub fn log(name: &str, config_list: &[String], message: &str) {
    if visible(name, config_list) {
        log::info!(target: "horizon_discordsrv", "[DiscordSRV] [{name} DEBUG] {message}");
    }
}

/// The words currently switched on (sorted), for tests and the report.
pub fn active() -> Vec<String> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

/// `/discord debugger [start|on|stop|off|upload] [categories... | lines]`; `&`-coded lines.
/// `facts` are the report's `name: value` lines (no IDs, no tokens, no log lines); `report_dir`
/// is where `upload` writes the report.
pub fn run(args: &[&str], console: bool, report_dir: &Path, facts: &[String]) -> Vec<String> {
    let (sub, rest) = args.split_first().map_or(("start", &[][..]), |(s, r)| (*s, r));
    let sub_lower = sub.to_ascii_lowercase();
    if sub_lower == "start" || sub_lower == "on" {
        let mut out = Vec::new();
        let mut accepted: Vec<String> = Vec::new();
        for arg in rest {
            if valid(arg) {
                accepted.push((*arg).to_string());
            } else {
                out.push(format!("&cInvalid debug category: &4{arg}"));
            }
        }
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if accepted.is_empty() {
            active.insert("UNCATEGORIZED".to_string());
        } else {
            active.extend(accepted);
        }
        out.push(
            "&3Debugger enabled, use &7/discordsrv debugger stop &3to stop debugging or &7/discordsrv debugger upload &3to stop debugging and generate a debug report"
                .to_string(),
        );
        return out;
    }
    let upload = sub_lower == "upload";
    if upload || sub_lower == "stop" || sub_lower == "off" {
        let mut out = Vec::new();
        if upload {
            // DiscordSRV's `Integer.parseInt(arguments.get(0))`: a non-number is an internal error.
            let lines = match rest.first() {
                None => Some(256usize),
                Some(n) => n.parse::<usize>().ok(),
            };
            match lines {
                Some(_) => match write_report(report_dir, if console { "CONSOLE" } else { "a player" }, facts) {
                    Ok(path) => out.push(format!("&3Your debug report has been generated and is available at &b{path}")),
                    Err(e) => out.push(format!("&cThe debug report could not be written: {e}")),
                },
                None => return vec!["&cAn internal error occurred while while processing your command.".to_string()],
            }
        } else {
            out.push("&3Debugger disabled".to_string());
        }
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clear();
        return out;
    }
    vec![format!("&cInvalid subcommand &4{sub}")]
}

/// Writes `debug-<unix seconds>.txt` into `dir/debug/` and returns its path.
fn write_report(dir: &Path, requested_by: &str, facts: &[String]) -> Result<String, String> {
    let folder = dir.join("debug");
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = folder.join(format!("debug-{stamp}.txt"));
    let mut text = format!("DiscordSRV debug report (horizon port)\nrequested by: {requested_by}\ncategories: {}\n", active().join(", "));
    for fact in facts {
        text.push_str(fact);
        text.push('\n');
    }
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_words_match_like_the_debug_enum() {
        assert!(valid("chat") && valid("MCTODISCORD") && valid("errors") && valid("all"));
        assert!(!valid("nope"));
        let (_, stack) = CATEGORIES.iter().find(|(n, _)| *n == "CALLSTACKS").unwrap();
        assert!(!matches("CALLSTACKS", stack, "all"), "all does not switch the call stacks on");
        assert!(matches("MINECRAFT_TO_DISCORD", &[], "chat") && !matches("GROUP_SYNC", &[], "chat"));
    }

    #[test]
    fn the_debugger_starts_stops_and_rejects_words() {
        let dir = std::env::temp_dir().join(format!("hdsrv-debugger-{}", std::process::id()));
        ACTIVE.lock().unwrap().clear();
        let out = run(&["start", "chat", "bogus"], true, &dir, &[]);
        assert_eq!(out[0], "&cInvalid debug category: &4bogus");
        assert!(out[1].starts_with("&3Debugger enabled, use &7/discordsrv debugger stop"));
        assert!(visible("MINECRAFT_TO_DISCORD", &[]) && !visible("GROUP_SYNC", &[]));
        assert!(visible("GROUP_SYNC", &["roles".to_string()]));
        assert_eq!(run(&["off"], true, &dir, &[]), vec!["&3Debugger disabled".to_string()]);
        assert!(active().is_empty());
        assert_eq!(run(&["start"], true, &dir, &[]).len(), 1);
        assert_eq!(active(), vec!["UNCATEGORIZED".to_string()]);
        assert_eq!(run(&["frobnicate"], true, &dir, &[]), vec!["&cInvalid subcommand &4frobnicate".to_string()]);
        assert_eq!(
            run(&["upload", "abc"], true, &dir, &[]),
            vec!["&cAn internal error occurred while while processing your command.".to_string()]
        );
        let out = run(&["upload"], true, &dir, &["version: x".to_string()]);
        assert!(out[0].starts_with("&3Your debug report has been generated and is available at &b"), "{out:?}");
        assert!(active().is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
