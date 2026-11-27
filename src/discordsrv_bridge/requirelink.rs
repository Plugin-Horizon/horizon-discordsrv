//! `Require linked account to play` (DiscordSRV's `RequireLinkModule.check`), in the order the
//! original asks: bypass names, vanilla whitelist, banned players, "still starting", the link,
//! membership of the Discord server(s), the subscriber roles. Each failure gives the kick text of
//! linking.yml (`&` codes; `{BOT}`, `{CODE}`, `{INVITE}` filled). The Discord questions go through
//! [`Discord`], so the rules are tested without a network.
//!
//! Not asked: the IP ban list (`Check banned players` / `Only check banned players` look at the
//! player's address in the original): Horizon's login hook gives no address, and behind the proxy
//! every login would come from the proxy's own address anyway, so only the name ban list counts.

use serde_json::Value;

use crate::discordsrv_bridge::config::RequireLinked;
use crate::discordsrv_bridge::{NOT_LINKED, STILL_STARTING, not_linked_text};

const NOT_IN_SERVER: &str = "&cYou are currently not a part of our Discord server.\n\nJoin at {INVITE}!";
const NO_SUBSCRIBER_ROLE: &str = "&cFailed to find any subscriber role on Discord.\n\nContact your server admins about this issue.";
const UNKNOWN_FAILURE: &str = "&cAn error occurred while trying to verify your account.\n\nContact your server admin about this issue.";
const NOT_SUBSCRIBED: &str = "&cYou must be subscribed on Twitch to be able to play.";

/// What the server knows of the player who logs in.
pub struct Facts<'a> {
    pub name: &'a str,
    /// On the vanilla whitelist.
    pub whitelisted: bool,
    /// On the name ban list.
    pub banned: bool,
    /// The gateway is connected.
    pub ready: bool,
    pub bot_name: &'a str,
    pub invite: &'a str,
}

/// The questions the check asks of Discord and of the link file.
pub trait Discord {
    /// The Discord account linked to the player.
    fn linked(&mut self) -> Option<String>;
    /// A fresh link code for the player.
    fn new_code(&mut self) -> String;
    /// The guilds the bot is in.
    fn bot_guilds(&self) -> Vec<String>;
    /// The guild a role belongs to, when the bot knows the role.
    fn role_guild(&self, role: &str) -> Option<String>;
    /// The user's role IDs in a guild; `Ok(None)` when the user is not a member.
    fn member_roles(&mut self, guild: &str, user: &str) -> Result<Option<Vec<String>>, String>;
}

fn or_default<'a>(text: &'a str, default: &'a str) -> &'a str {
    if text.trim().is_empty() { default } else { text }
}

/// Whether `Whitelisted players bypass check` applies to this player (the key is on by default).
pub fn whitelist_bypasses(rule: &RequireLinked, whitelisted: bool) -> bool {
    rule.whitelisted_players_bypass_check.unwrap_or(true) && whitelisted
}

/// The kick text of a login that must be refused; None lets the player in. A Discord error is a
/// refusal with the "unknown reason" text and a line in `log`, as the original's catch-all.
pub fn decide(rule: &RequireLinked, f: &Facts<'_>, d: &mut dyn Discord, log: &mut Vec<String>) -> Option<String> {
    if !rule.enabled || rule.bypass_names.iter().any(|b| b.as_str() == f.name) {
        return None;
    }
    if whitelist_bypasses(rule, f.whitelisted) {
        log.push(format!("Player {} is bypassing link requirement, player is whitelisted", f.name));
        return None;
    }
    let only_banned = rule.only_check_banned_players;
    if !rule.check_banned_players || only_banned {
        if f.banned && !only_banned {
            log.push(format!("Player {} is banned, skipping linked check", f.name));
            return None;
        }
        if only_banned && !f.banned {
            log.push(format!("Player {} is bypassing link requirement because \"Only check banned players\" is enabled", f.name));
            return None;
        }
    }
    if !f.ready {
        return Some(or_default(&rule.messages.still_starting, STILL_STARTING).to_string());
    }
    let Some(discord) = d.linked() else {
        let code = d.new_code();
        return Some(not_linked_text(or_default(&rule.not_linked_message, NOT_LINKED), f.bot_name, &code, f.invite));
    };
    match after_link(rule, f, d, &discord, log) {
        Ok(None) => None,
        Ok(Some(text)) => Some(text),
        Err(e) => {
            log.push(format!("Failed to check player: {} ({e})", f.name));
            Some(or_default(&rule.messages.failed_for_unknown_reason, UNKNOWN_FAILURE).to_string())
        }
    }
}

fn after_link(rule: &RequireLinked, f: &Facts<'_>, d: &mut dyn Discord, discord: &str, log: &mut Vec<String>) -> Result<Option<String>, String> {
    let not_in_server = || or_default(&rule.messages.not_in_server, NOT_IN_SERVER).replace("{INVITE}", f.invite);
    // `Must be in Discord server`: true (any guild the bot is in), false, one ID, or a list of IDs.
    let must = &rule.must_be_in_discord_server;
    match must {
        Value::Bool(false) => {}
        Value::Null | Value::Bool(true) => {
            let mut present = false;
            for guild in d.bot_guilds() {
                if d.member_roles(&guild, discord)?.is_some() {
                    present = true;
                    break;
                }
            }
            if !present {
                log.push(format!("Player {}'s linked Discord account is NOT present, denying login", f.name));
                return Ok(Some(not_in_server()));
            }
        }
        other => {
            let ids: Vec<String> = match other {
                Value::Array(a) => a.iter().map(plain).collect(),
                one => vec![plain(one)],
            };
            let known = d.bot_guilds();
            for id in ids {
                if !known.contains(&id) {
                    log.push(format!("Failed to get Discord server by ID {id}: bot is not in server"));
                    continue;
                }
                if d.member_roles(&id, discord)?.is_none() {
                    log.push(format!("Player {}'s linked Discord account is NOT present, denying login", f.name));
                    return Ok(Some(not_in_server()));
                }
            }
        }
    }
    let sub = &rule.subscriber_role;
    if sub.required && !sub.roles.is_empty() {
        let (mut failed, mut matches) = (0, 0);
        for role in &sub.roles {
            let Some(guild) = (!role.trim().is_empty()).then(|| d.role_guild(role)).flatten() else {
                failed += 1;
                continue;
            };
            if d.member_roles(&guild, discord)?.is_some_and(|have| have.contains(role)) {
                matches += 1;
            }
        }
        if failed == sub.roles.len() {
            log.push(format!(
                "Tried to authenticate {} but no valid subscriber role IDs are found and thats a requirement; login will be denied until this is fixed.",
                f.name
            ));
            return Ok(Some(or_default(&rule.messages.failed_to_find_subscriber_role, NO_SUBSCRIBER_ROLE).to_string()));
        }
        let denied = if sub.require_all { matches < sub.roles.len() } else { matches == 0 };
        if denied {
            log.push(format!("Player {} does NOT match subscriber role requirements, denying login", f.name));
            return Ok(Some(or_default(&sub.kick_message, NOT_SUBSCRIBED).to_string()));
        }
    }
    Ok(None)
}

/// An ID written as text or as a number.
fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `noticePlayerUnlink`: does a player who unlinked get kicked? Not when the whitelist bypass holds or
/// when only banned players need to link and this one is not banned.
pub fn unlink_kick_applies(rule: &RequireLinked, whitelisted: bool, banned: bool) -> bool {
    if whitelist_bypasses(rule, whitelisted) {
        return false;
    }
    !(rule.only_check_banned_players && !banned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        linked: Option<String>,
        guilds: Vec<String>,
        /// (guild, user) -> roles
        members: HashMap<(String, String), Vec<String>>,
        roles: HashMap<String, String>,
        fail: bool,
        codes: u32,
    }

    impl Discord for Fake {
        fn linked(&mut self) -> Option<String> {
            self.linked.clone()
        }
        fn new_code(&mut self) -> String {
            self.codes += 1;
            "1234".into()
        }
        fn bot_guilds(&self) -> Vec<String> {
            self.guilds.clone()
        }
        fn role_guild(&self, role: &str) -> Option<String> {
            self.roles.get(role).cloned()
        }
        fn member_roles(&mut self, guild: &str, user: &str) -> Result<Option<Vec<String>>, String> {
            if self.fail {
                return Err("HTTP 500".into());
            }
            Ok(self.members.get(&(guild.to_string(), user.to_string())).cloned())
        }
    }

    fn rule(extra: Value) -> RequireLinked {
        let mut base = json!({ "Enabled": true, "Bypass names": ["Admin"], "Not linked message": "link {BOT} {CODE} {INVITE}" });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn facts<'a>(name: &'a str) -> Facts<'a> {
        Facts { name, whitelisted: false, banned: false, ready: true, bot_name: "Bot", invite: "https://d.example/i" }
    }

    fn member_of(f: &mut Fake, guild: &str, roles: &[&str]) {
        f.guilds.push(guild.into());
        f.members.insert((guild.into(), "42".into()), roles.iter().map(|r| r.to_string()).collect());
    }

    #[test]
    fn unlinked_players_get_the_code() {
        let mut d = Fake::default();
        let r = rule(json!({}));
        let t = decide(&r, &facts("Steve"), &mut d, &mut Vec::new()).unwrap();
        assert_eq!(t, "link Bot 1234 https://d.example/i");
        // Off, or on the bypass list: let in.
        assert!(decide(&RequireLinked::default(), &facts("Steve"), &mut d, &mut Vec::new()).is_none());
        assert!(decide(&r, &facts("Admin"), &mut d, &mut Vec::new()).is_none());
        assert_eq!(d.codes, 1);
    }

    #[test]
    fn whitelisted_and_banned_players() {
        let mut d = Fake::default();
        let mut w = facts("Steve");
        w.whitelisted = true;
        assert!(decide(&rule(json!({})), &w, &mut d, &mut Vec::new()).is_none());
        assert!(decide(&rule(json!({ "Whitelisted players bypass check": false })), &w, &mut d, &mut Vec::new()).is_some());
        // A banned player skips the check unless banned players are checked too.
        let mut b = facts("Steve");
        b.banned = true;
        assert!(decide(&rule(json!({})), &b, &mut d, &mut Vec::new()).is_none());
        assert!(decide(&rule(json!({ "Check banned players": true })), &b, &mut d, &mut Vec::new()).is_some());
        // Only banned players need to link.
        let only = rule(json!({ "Only check banned players": true }));
        assert!(decide(&only, &facts("Steve"), &mut d, &mut Vec::new()).is_none());
        assert!(decide(&only, &b, &mut d, &mut Vec::new()).is_some());
    }

    #[test]
    fn a_login_before_the_gateway_is_ready_is_refused() {
        let mut f = facts("Steve");
        f.ready = false;
        let t = decide(&rule(json!({})), &f, &mut Fake::default(), &mut Vec::new()).unwrap();
        assert!(t.starts_with("&cCurrently unavailable"));
        let custom = rule(json!({ "Messages": { "DiscordSRV still starting": "wait" } }));
        assert_eq!(decide(&custom, &f, &mut Fake::default(), &mut Vec::new()).as_deref(), Some("wait"));
    }

    #[test]
    fn membership_true_false_one_and_many() {
        let mut d = Fake { linked: Some("42".into()), ..Default::default() };
        d.guilds = vec!["g1".into(), "g2".into()];
        let mut log = Vec::new();
        // true: member of no guild the bot is in.
        let r = rule(json!({ "Must be in Discord server": true, "DiscordInviteLink": "x" }));
        let t = decide(&r, &facts("Steve"), &mut d, &mut log).unwrap();
        assert_eq!(t, "&cYou are currently not a part of our Discord server.\n\nJoin at https://d.example/i!");
        // false: only the link counts.
        assert!(decide(&rule(json!({ "Must be in Discord server": false })), &facts("Steve"), &mut d, &mut log).is_none());
        // Member of g2: true is met.
        d.members.insert(("g2".into(), "42".into()), vec![]);
        assert!(decide(&r, &facts("Steve"), &mut d, &mut log).is_none());
        // One ID, and a list that needs both.
        assert!(decide(&rule(json!({ "Must be in Discord server": "g2" })), &facts("Steve"), &mut d, &mut log).is_none());
        assert!(decide(&rule(json!({ "Must be in Discord server": "g1" })), &facts("Steve"), &mut d, &mut log).is_some());
        assert!(decide(&rule(json!({ "Must be in Discord server": ["g1", "g2"] })), &facts("Steve"), &mut d, &mut log).is_some());
        // A server the bot is not in is skipped with a note.
        assert!(decide(&rule(json!({ "Must be in Discord server": ["g2", "g9"] })), &facts("Steve"), &mut d, &mut log).is_none());
        assert!(log.iter().any(|l| l.contains("g9")));
    }

    #[test]
    fn subscriber_roles_any_all_and_missing() {
        let mut d = Fake { linked: Some("42".into()), ..Default::default() };
        member_of(&mut d, "g", &["r1"]);
        d.roles.insert("r1".into(), "g".into());
        d.roles.insert("r2".into(), "g".into());
        let sub = |all: bool, roles: Value| {
            rule(json!({ "Must be in Discord server": false, "Subscriber role": {
                "Require subscriber role to join": true, "Subscriber roles": roles,
                "Require all of the listed roles": all, "Kick message": "&cnope" } }))
        };
        let mut log = Vec::new();
        assert!(decide(&sub(false, json!(["r1", "r2"])), &facts("Steve"), &mut d, &mut log).is_none());
        assert_eq!(decide(&sub(true, json!(["r1", "r2"])), &facts("Steve"), &mut d, &mut log).as_deref(), Some("&cnope"));
        assert_eq!(decide(&sub(false, json!(["r2"])), &facts("Steve"), &mut d, &mut log).as_deref(), Some("&cnope"));
        // No role ID the bot knows (or blank ones): the config error text.
        let t = decide(&sub(false, json!(["", "zzz"])), &facts("Steve"), &mut d, &mut log).unwrap();
        assert!(t.starts_with("&cFailed to find any subscriber role"));
        // One unknown role next to a held one still lets "any" through.
        assert!(decide(&sub(false, json!(["zzz", "r1"])), &facts("Steve"), &mut d, &mut log).is_none());
    }

    #[test]
    fn a_discord_error_denies_with_the_unknown_reason() {
        let mut d = Fake { linked: Some("42".into()), fail: true, guilds: vec!["g".into()], ..Default::default() };
        let mut log = Vec::new();
        let t = decide(&rule(json!({})), &facts("Steve"), &mut d, &mut log).unwrap();
        assert!(t.starts_with("&cAn error occurred"));
        assert!(log[0].contains("Failed to check player: Steve"));
    }

    #[test]
    fn the_unlink_kick_follows_the_same_exceptions() {
        let r = rule(json!({}));
        assert!(unlink_kick_applies(&r, false, false));
        assert!(!unlink_kick_applies(&r, true, false));
        let only = rule(json!({ "Only check banned players": true }));
        assert!(!unlink_kick_applies(&only, false, false));
        assert!(unlink_kick_applies(&only, false, true));
    }
}
