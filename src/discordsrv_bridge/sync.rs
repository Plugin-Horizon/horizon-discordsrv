//! Group -> role synchronisation (`GroupRoleSynchronization*`, `/discord resync`) and the ban
//! synchronisation helpers. Minecraft is the authority: Discord -> Minecraft group changes need a
//! permission provider's write API, which a mod does not have (see the README).
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::discordsrv_bridge::Shared;

/// What the permission provider published for one player.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Groups {
    pub all: Vec<String>,
    pub primary: Option<String>,
}

/// A role id that names no role: empty, or only zeros (the example config's placeholders).
fn blank_role(id: &str) -> bool {
    id.replace('0', "").trim().is_empty()
}

/// The roles to give and take so a member's roles follow the player's groups
/// (`GroupRoleSynchronizationGroupsAndRolesToSync`, Minecraft authoritative). A group counts when
/// the player is in it (case-insensitive), or is the primary group with `primary_only`.
pub fn plan(
    map: &HashMap<String, String>,
    groups: &Groups,
    primary_only: bool,
    member_roles: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut add = Vec::new();
    let mut remove = Vec::new();
    let mut entries: Vec<(&String, &String)> = map.iter().collect();
    entries.sort();
    for (group, role) in entries {
        if group.trim().is_empty() || blank_role(role) {
            continue;
        }
        let has_group = if primary_only {
            groups.primary.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(group))
        } else {
            groups.all.iter().any(|g| g.eq_ignore_ascii_case(group))
        };
        let has_role = member_roles.iter().any(|r| r == role);
        if has_group && !has_role {
            add.push(role.clone());
        } else if !has_group && has_role {
            remove.push(role.clone());
        }
    }
    (add, remove)
}

/// The nickname format filled in and cut to Discord's 32 characters.
pub fn nickname(format: &str, name: &str) -> String {
    format.replace("%username%", name).replace("%displayname%", name).chars().take(32).collect()
}

static LAST_CYCLE_MS: AtomicU64 = AtomicU64::new(0);

/// Whether a synchronisation of everyone is due: `minutes` (at least 1) after the last one.
/// The first call arms the timer and answers false.
pub fn cycle_due(minutes: u64) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let last = LAST_CYCLE_MS.load(Ordering::Relaxed);
    if last == 0 {
        LAST_CYCLE_MS.store(now, Ordering::Relaxed);
        return false;
    }
    if now.saturating_sub(last) >= minutes.max(1) * 60_000 {
        LAST_CYCLE_MS.store(now, Ordering::Relaxed);
        return true;
    }
    false
}

impl Shared {
    /// `isGroupRoleSynchronizationEnabled`: at least one group has a real role ID.
    pub(crate) fn sync_enabled(&self) -> bool {
        self.config
            .chat
            .group_role_synchronization_groups_and_roles_to_sync
            .iter()
            .any(|(group, role)| !group.trim().is_empty() && !blank_role(role))
    }

    /// `NicknameSynchronization*`: each linked member's nickname becomes the formatted player name.
    fn sync_nicknames(&self, players: &HashMap<u128, Groups>) {
        let c = &self.config.chat;
        if !c.nickname_synchronization_enabled {
            return;
        }
        for uuid in players.keys() {
            let (discord, name, guild) = {
                let st = self.lock_state();
                let Some(discord) = st.accounts.discord_of(*uuid).map(str::to_string) else { continue };
                let Some(name) = crate::discordsrv_bridge::commands::known_name(&st, &self.server, *uuid) else { continue };
                let guild = c.channel_for("global").and_then(|ch| st.cache.guild_of_channel(ch)).map(|g| g.id.clone());
                (discord, name, guild)
            };
            let Some(guild) = guild else { continue };
            let nick = nickname(&c.nickname_synchronization_format, &name);
            if let Err(e) = self.rest.set_nick(&guild, &discord, &nick) {
                log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Nickname synchronization: {e}");
            }
        }
    }

    /// The UUIDs of every linked player.
    pub(crate) fn linked_uuids(&self) -> Vec<u128> {
        self.lock_state().accounts.all_uuids()
    }

    /// Synchronises the given players; returns how many Discord members got a change.
    /// Network calls: runs on the chat thread, never the server thread.
    pub(crate) fn resync_players(&self, players: &HashMap<u128, Groups>) -> usize {
        let c = &self.config.chat;
        self.sync_nicknames(players);
        // Minecraft -> Discord only: a Discord-authoritative setup changes nothing from here.
        if !self.sync_enabled() || !c.group_role_synchronization_minecraft_is_authoritative {
            return 0;
        }
        let mut changed = 0;
        for (uuid, groups) in players {
            let (discord, guild_of_role) = {
                let st = self.lock_state();
                let Some(discord) = st.accounts.discord_of(*uuid).map(str::to_string) else { continue };
                // role id -> guild id
                let mut owners: HashMap<String, String> = HashMap::new();
                for guild in st.cache.guilds.values() {
                    for role in guild.roles.keys() {
                        owners.insert(role.clone(), guild.id.clone());
                    }
                }
                (discord, owners)
            };
            // Per guild: the member's roles from Discord, then the difference.
            let mut guilds: Vec<&String> = guild_of_role.values().collect();
            guilds.sort();
            guilds.dedup();
            for guild in guilds {
                let in_guild: HashMap<String, String> = c
                    .group_role_synchronization_groups_and_roles_to_sync
                    .iter()
                    .filter(|(_, role)| guild_of_role.get(*role) == Some(guild))
                    .map(|(g, r)| (g.clone(), r.clone()))
                    .collect();
                if in_guild.is_empty() {
                    continue;
                }
                let roles = match self.rest.member_roles(guild, &discord) {
                    Ok(Some(roles)) => roles,
                    Ok(None) => continue,
                    Err(e) => {
                        log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Group synchronization: {e}");
                        continue;
                    }
                };
                let (add, remove) = plan(&in_guild, groups, c.group_role_synchronization_primary_group_only, &roles);
                for role in &add {
                    if let Err(e) = self.rest.add_role(guild, &discord, role) {
                        log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Group synchronization failed: {e}");
                    }
                }
                for role in &remove {
                    if let Err(e) = self.rest.remove_role(guild, &discord, role) {
                        log::debug!(target: "horizon_discordsrv", "[DiscordSRV] Group synchronization failed: {e}");
                    }
                }
                if !add.is_empty() || !remove.is_empty() {
                    changed += 1;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> HashMap<String, String> {
        HashMap::from([
            ("trusted".to_string(), "111".to_string()),
            ("vip".to_string(), "222".to_string()),
            ("unset".to_string(), "000000000000000000".to_string()),
            ("".to_string(), "333".to_string()),
        ])
    }

    #[test]
    fn roles_follow_groups_and_skip_placeholders() {
        let groups = Groups { all: vec!["Trusted".into(), "default".into()], primary: Some("default".into()) };
        let (add, remove) = plan(&map(), &groups, false, &["222".to_string()]);
        assert_eq!(add, vec!["111".to_string()]);
        assert_eq!(remove, vec!["222".to_string()]);
    }

    #[test]
    fn primary_group_only_counts_the_primary() {
        let groups = Groups { all: vec!["trusted".into(), "vip".into()], primary: Some("vip".into()) };
        let (add, remove) = plan(&map(), &groups, true, &["111".to_string()]);
        assert_eq!(add, vec!["222".to_string()]);
        assert_eq!(remove, vec!["111".to_string()]);
    }

    #[test]
    fn nothing_changes_when_both_sides_agree() {
        let groups = Groups { all: vec!["vip".into()], primary: None };
        let (add, remove) = plan(&map(), &groups, false, &["222".to_string()]);
        assert!(add.is_empty() && remove.is_empty());
    }

    #[test]
    fn nicknames_follow_the_format_and_are_cut() {
        assert_eq!(nickname("%displayname%", "Tester"), "Tester");
        assert_eq!(nickname("[MC] %username%", "Tester"), "[MC] Tester");
        assert_eq!(nickname("%username%%username%%username%%username%", "Abcdefgh").chars().count(), 32);
    }

    #[test]
    fn placeholder_ids_are_blank() {
        assert!(blank_role("000000000000000000"));
        assert!(blank_role(""));
        assert!(!blank_role("100"));
    }
}
