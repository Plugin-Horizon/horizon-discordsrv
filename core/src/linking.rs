//! Account linking: `/discord link` hands a player a four-digit code, the code sent to the bot
//! links their Discord account. Links are kept in `accounts.aof` beside the config, one
//! `<discord id> <uuid>` line each (see STATUS.md: the format DiscordSRV itself writes is still
//! to be compared with a file it made).
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::PathBuf;

use crate::server::{parse_uuid, uuid_string};

pub struct Accounts {
    path: PathBuf,
    /// Discord ID -> player UUID.
    links: HashMap<String, u128>,
    /// Code -> (player UUID, player name).
    codes: HashMap<String, (u128, String)>,
}

/// What a code sent to the bot did.
#[derive(Debug, PartialEq, Eq)]
pub enum CodeResult {
    /// Not four digits.
    Invalid,
    /// Four digits nobody was given.
    Unknown,
    /// This Discord account is linked already (and relinking is off).
    AlreadyLinked(u128),
    /// Linked now; `replaced` is the player it was linked to before (relink).
    Linked { uuid: u128, name: String, replaced: Option<u128> },
}

impl Accounts {
    /// Reads the file (missing: no links). Lines that don't parse are skipped; a later line for
    /// the same Discord account or player wins.
    pub fn load(path: PathBuf) -> Accounts {
        let mut links = HashMap::new();
        for line in fs::read_to_string(&path).unwrap_or_default().lines() {
            // DiscordSRV's append-only file: `<discord id> <uuid>` links, and `-<discord id>`,
            // `-<uuid>` or `-<discord id> <uuid>` lines that take a link away again.
            if let Some(rest) = line.strip_prefix('-') {
                for word in rest.split_whitespace().take(2) {
                    if word.len() >= 17 && word.chars().all(|c| c.is_ascii_digit()) {
                        links.remove(word);
                    } else if word.contains('-')
                        && let Some(uuid) = parse_uuid(word)
                    {
                        links.retain(|_, u| *u != uuid);
                    }
                }
                continue;
            }
            let mut parts = line.split_whitespace();
            if let (Some(id), Some(uuid)) = (parts.next(), parts.next().and_then(parse_uuid))
                && id.chars().all(|c| c.is_ascii_digit())
            {
                links.retain(|_, u| *u != uuid);
                links.insert(id.to_string(), uuid);
            }
        }
        Accounts { path, links, codes: HashMap::new() }
    }

    pub fn uuid_of(&self, discord: &str) -> Option<u128> {
        self.links.get(discord).copied()
    }

    pub fn discord_of(&self, uuid: u128) -> Option<&str> {
        self.links.iter().find(|(_, u)| **u == uuid).map(|(id, _)| id.as_str())
    }

    /// A new code for this player (their earlier one stops working).
    pub fn new_code(&mut self, uuid: u128, name: &str) -> String {
        self.codes.retain(|_, (u, _)| *u != uuid);
        let state = RandomState::new();
        let mut salt = 0u64;
        loop {
            let mut h = state.build_hasher();
            h.write_u128(uuid);
            h.write_u64(salt);
            let code = format!("{:04}", h.finish() % 10_000);
            if !self.codes.contains_key(&code) {
                self.codes.insert(code.clone(), (uuid, name.to_string()));
                return code;
            }
            salt += 1;
        }
    }

    /// Forgets the codes given to a player.
    pub fn drop_codes(&mut self, uuid: u128) {
        self.codes.retain(|_, (u, _)| *u != uuid);
    }

    /// A message sent to the bot as a code. As in DiscordSRV everything but digits is dropped from
    /// the message; four digits nobody was given are unknown, anything else is invalid.
    pub fn redeem(&mut self, discord: &str, text: &str, allow_relink: bool) -> Result<CodeResult, String> {
        if let Some(uuid) = self.uuid_of(discord)
            && !allow_relink
        {
            return Ok(CodeResult::AlreadyLinked(uuid));
        }
        let code: String = text.chars().filter(char::is_ascii_digit).collect();
        let Some((uuid, name)) = self.codes.remove(&code) else {
            return Ok(if code.len() == 4 { CodeResult::Unknown } else { CodeResult::Invalid });
        };
        let replaced = self.uuid_of(discord);
        self.links.retain(|_, u| *u != uuid);
        self.links.insert(discord.to_string(), uuid);
        self.save()?;
        Ok(CodeResult::Linked { uuid, name, replaced })
    }

    /// Links a player to a Discord account without a code (staff): the player's earlier link and
    /// the account's earlier one go.
    pub fn link(&mut self, uuid: u128, discord: &str) -> Result<(), String> {
        self.links.retain(|id, u| *u != uuid && id.as_str() != discord);
        self.links.insert(discord.to_string(), uuid);
        self.save()
    }

    /// Unlinks a player; their Discord ID when they were linked.
    pub fn unlink(&mut self, uuid: u128) -> Result<Option<String>, String> {
        let Some(id) = self.discord_of(uuid).map(str::to_string) else {
            return Ok(None);
        };
        self.links.remove(&id);
        self.save()?;
        Ok(Some(id))
    }

    /// Unlinks a Discord account; the player's UUID when it was linked.
    pub fn unlink_discord(&mut self, discord: &str) -> Result<Option<u128>, String> {
        let Some(uuid) = self.links.remove(discord) else {
            return Ok(None);
        };
        self.save()?;
        Ok(Some(uuid))
    }

    /// The whole file again (small: one line a linked player), through a temporary file.
    fn save(&self) -> Result<(), String> {
        let mut lines: Vec<String> = self.links.iter().map(|(id, u)| format!("{id} {}", uuid_string(*u))).collect();
        lines.sort();
        let tmp = self.path.with_extension("aof.tmp");
        let write = || -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            for l in &lines {
                writeln!(f, "{l}")?;
            }
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        };
        write().map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hdsrv-link-{name}-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_code_links_once_and_is_saved() {
        let d = dir("a");
        let mut a = Accounts::load(d.join("accounts.aof"));
        let code = a.new_code(7, "alex");
        assert_eq!(code.len(), 4);
        assert_eq!(a.redeem("111", "12", false).unwrap(), CodeResult::Invalid);
        let wrong = if code == "0000" { "0001" } else { "0000" };
        assert_eq!(a.redeem("111", wrong, false).unwrap(), CodeResult::Unknown);
        assert_eq!(
            a.redeem("111", &format!(" {code} "), false).unwrap(),
            CodeResult::Linked { uuid: 7, name: "alex".into(), replaced: None }
        );
        assert_eq!(a.redeem("111", &code, false).unwrap(), CodeResult::AlreadyLinked(7));
        let again = Accounts::load(d.join("accounts.aof"));
        assert_eq!(again.uuid_of("111"), Some(7));
        assert_eq!(again.discord_of(7), Some("111"));
        assert_eq!(
            fs::read_to_string(d.join("accounts.aof")).unwrap(),
            "111 00000000-0000-0000-0000-000000000007\n"
        );
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn relinking_and_unlinking() {
        let d = dir("b");
        let mut a = Accounts::load(d.join("accounts.aof"));
        let c1 = a.new_code(7, "alex");
        a.redeem("111", &c1, false).unwrap();
        let c2 = a.new_code(8, "steve");
        assert_eq!(
            a.redeem("111", &c2, true).unwrap(),
            CodeResult::Linked { uuid: 8, name: "steve".into(), replaced: Some(7) }
        );
        assert_eq!(a.discord_of(7), None);
        assert_eq!(a.unlink(8).unwrap(), Some("111".into()));
        assert_eq!(a.unlink(8).unwrap(), None);
        assert_eq!(Accounts::load(d.join("accounts.aof")).uuid_of("111"), None);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn unlink_lines_of_the_append_only_file_take_links_away() {
        let d = dir("c");
        fs::write(
            d.join("accounts.aof"),
            "111111111111111111 00000000-0000-0000-0000-000000000007\n\
             222222222222222222 00000000-0000-0000-0000-000000000008\n\
             -111111111111111111 00000000-0000-0000-0000-000000000007\n",
        )
        .unwrap();
        let mut a = Accounts::load(d.join("accounts.aof"));
        assert_eq!(a.uuid_of("111111111111111111"), None);
        assert_eq!(a.uuid_of("222222222222222222"), Some(8));
        assert_eq!(a.unlink_discord("222222222222222222").unwrap(), Some(8));
        assert_eq!(a.unlink_discord("222222222222222222").unwrap(), None);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_new_code_replaces_the_old_one() {
        let mut a = Accounts::load(PathBuf::from("/nonexistent/accounts.aof"));
        let first = a.new_code(7, "alex");
        let second = a.new_code(7, "alex");
        assert_eq!(a.codes.len(), 1);
        assert!(a.codes.contains_key(&second));
        let _ = first;
    }
}
