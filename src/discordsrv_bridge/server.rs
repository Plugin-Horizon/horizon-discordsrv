//! What the bridge knows of the server beside the hooks: `server.properties` (max players,
//! MOTD, level name), the players seen on the last snapshot, and the world's player files
//! (counted for `%totalplayers%`, looked for to tell a first join). Files are only listed, never
//! read.
use std::fs;
use std::path::{Path, PathBuf};

/// A player on the server, as the tick hands them over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub uuid: u128,
    pub name: String,
    /// `minecraft:overworld`, `minecraft:the_nether`...
    pub dimension: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub home: PathBuf,
    pub max_players: u32,
    pub motd: String,
    pub level_name: String,
}

impl ServerInfo {
    /// Reads `<home>/server.properties` (vanilla's defaults where it says nothing).
    pub fn load(home: &Path) -> ServerInfo {
        let text = fs::read_to_string(home.join("server.properties")).unwrap_or_default();
        Self::parse(home, &text)
    }

    pub fn parse(home: &Path, text: &str) -> ServerInfo {
        let get = |key: &str| {
            text.lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_string())
        };
        ServerInfo {
            home: home.to_path_buf(),
            max_players: get("max-players").and_then(|v| v.parse().ok()).unwrap_or(20),
            motd: unescape_properties(&get("motd").unwrap_or_else(|| "A Minecraft Server".into())),
            level_name: get("level-name").filter(|v| !v.is_empty()).unwrap_or_else(|| "world".into()),
        }
    }

    fn player_dir(&self) -> PathBuf {
        self.home.join(&self.level_name).join("players").join("data")
    }

    /// Players who ever joined: the world's player files.
    pub fn total_players(&self) -> usize {
        fs::read_dir(self.player_dir())
            .map(|d| d.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".dat")).count())
            .unwrap_or(0)
    }

    /// Whether the world has no file of this player yet. Asked at login, before their state is
    /// loaded (GAPS.md: there is no "played before" helper).
    pub fn is_new_player(&self, uuid: u128) -> bool {
        !self.player_dir().join(format!("{}.dat", uuid_string(uuid))).exists()
    }

    /// Players the server knows by name: vanilla's `usercache.json` (`[{"name", "uuid", ...}]`),
    /// read each time (it is small and the server rewrites it).
    pub fn cached_players(&self) -> Vec<(u128, String)> {
        let text = fs::read_to_string(self.home.join("usercache.json")).unwrap_or_default();
        let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|e| Some((parse_uuid(e["uuid"].as_str()?)?, e["name"].as_str()?.to_string())))
            .collect()
    }

    /// The Bukkit world name of a dimension: `world`, `world_nether`, `world_the_end`.
    pub fn world_name(&self, dimension: &str) -> String {
        match dimension {
            "minecraft:overworld" | "" => self.level_name.clone(),
            "minecraft:the_nether" => format!("{}_nether", self.level_name),
            "minecraft:the_end" => format!("{}_the_end", self.level_name),
            other => other.rsplit(':').next().unwrap_or(other).to_string(),
        }
    }
}

/// `\:`, `\=`, `\\` and `\uXXXX` of a properties value.
fn unescape_properties(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// A UUID as `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
pub fn uuid_string(uuid: u128) -> String {
    let h = format!("{uuid:032x}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A dashed or plain UUID.
pub fn parse_uuid(text: &str) -> Option<u128> {
    let hex: String = text.trim().chars().filter(|c| *c != '-').collect();
    (hex.len() == 32).then(|| u128::from_str_radix(&hex, 16).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_give_max_players_motd_and_level() {
        let s = ServerInfo::parse(Path::new("/srv"), "#c\nmax-players=50\nmotd=Hello \\u00A7aWorld\\!\nlevel-name=\n");
        assert_eq!(s.max_players, 50);
        assert_eq!(s.motd, "Hello §aWorld!");
        assert_eq!(s.level_name, "world");
        assert_eq!(s.world_name("minecraft:the_nether"), "world_nether");
        assert_eq!(s.world_name("mymod:moon"), "moon");
        let d = ServerInfo::parse(Path::new("/srv"), "");
        assert_eq!((d.max_players, d.motd.as_str()), (20, "A Minecraft Server"));
    }

    #[test]
    fn uuids_round_trip() {
        let u = 0x0123456789abcdef0123456789abcdefu128;
        assert_eq!(uuid_string(u), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(parse_uuid("01234567-89ab-cdef-0123-456789abcdef"), Some(u));
        assert_eq!(parse_uuid("0123456789abcdef0123456789abcdef"), Some(u));
        assert_eq!(parse_uuid("nope"), None);
    }

    #[test]
    fn player_files_are_counted_and_looked_for() {
        let home = std::env::temp_dir().join(format!("hdsrv-server-{}", std::process::id()));
        let dir = home.join("world/players/data");
        fs::create_dir_all(&dir).unwrap();
        let u = 0x0123456789abcdef0123456789abcdefu128;
        fs::write(dir.join(format!("{}.dat", uuid_string(u))), b"").unwrap();
        fs::write(dir.join("other.dat_old"), b"").unwrap();
        let s = ServerInfo::parse(&home, "");
        assert_eq!(s.total_players(), 1);
        assert!(!s.is_new_player(u));
        assert!(s.is_new_player(u + 1));
        fs::remove_dir_all(&home).unwrap();
    }
}
