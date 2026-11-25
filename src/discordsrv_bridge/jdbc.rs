//! The JDBC account-link backend (`Experiment_JdbcAccountLinkBackend`, `JdbcAccountLinkManager`): the links
//! live in a MySQL / MariaDB table shared by every server that uses the same database, with the original's layout
//! (`<prefix>_accounts`: `link` int auto increment, `discord` varchar(32) unique, `uuid` varchar(36) unique; and the
//! `<prefix>_codes` table the original creates). The URL, the table names and the SQL are plain functions, tested
//! without a database; [`MysqlStore`] runs them.
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder};

use crate::discordsrv_bridge::config::Config;
use crate::discordsrv_bridge::linking::LinkStore;

/// The parts of `jdbc:mysql://host[:port]/database[?params]` the connection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub database: String,
}

/// Parses the URL as `JdbcAccountLinkManager.shouldUseJdbc` does. `Ok(None)`: the file's placeholder (`HOST`,
/// `PORT`, `DATABASE`) or an empty value, so JDBC is not wanted; `Err`: it is wanted but wrong.
pub fn parse_url(url: &str) -> Result<Option<Target>, String> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    let mut parts = url.splitn(3, ':');
    let (proto, engine, rest) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if !proto.eq_ignore_ascii_case("jdbc") {
        return Err("the protocol of the JDBC URL is wrong".into());
    }
    if !engine.eq_ignore_ascii_case("mysql") {
        return Err("only MySQL is supported for JDBC".into());
    }
    let rest = rest.strip_prefix("//").ok_or("the JDBC URL has no host")?;
    let (authority, path) = rest.split_once('/').ok_or("the JDBC URL has no database")?;
    let database = path.split('?').next().unwrap_or("").to_string();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.to_string()),
        None => (authority.to_string(), "3306".to_string()),
    };
    if host.eq_ignore_ascii_case("host") || port.eq_ignore_ascii_case("port") || database.eq_ignore_ascii_case("database") {
        return Ok(None);
    }
    if host.is_empty() || database.is_empty() || !database.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err("the JDBC URL is invalid".into());
    }
    let port = port.parse::<u16>().map_err(|_| "the JDBC port is not a number".to_string())?;
    Ok(Some(Target { host, port, database }))
}

/// The names of the accounts and codes tables: `<prefix>_accounts`, or `accounts` without a prefix.
pub fn table_names(prefix: &str) -> Result<(String, String), String> {
    let prefix = prefix.trim();
    if !prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Experiment_JdbcTablePrefix may hold letters, digits and _ only".into());
    }
    let p = if prefix.is_empty() { String::new() } else { format!("{prefix}_") };
    Ok((format!("{p}accounts"), format!("{p}codes")))
}

pub fn create_accounts_sql(table: &str) -> String {
    format!(
        "create table if not exists {table} (link int auto_increment primary key, discord varchar(32) not null, uuid varchar(36) not null, \
         constraint accounts_discord_uindex unique (discord), constraint accounts_uuid_uindex unique (uuid))"
    )
}

pub fn create_codes_sql(table: &str) -> String {
    format!("create table if not exists {table} (code char(4) not null primary key, uuid varchar(36) not null, expiration bigint(20) not null, constraint codes_uuid_uindex unique (uuid))")
}

pub fn upsert_sql(table: &str) -> String {
    format!("insert into {table} (discord, uuid) values (?, ?) on duplicate key update uuid = values(uuid)")
}

/// The store of the configured database, or `None` when JDBC is not wanted. The connection is made now: an error
/// (wrong URL, no server, wrong login) is the caller's to log, and the file backend is used instead.
pub fn connect(config: &Config) -> Result<Option<Box<dyn LinkStore>>, String> {
    let Some(target) = parse_url(&config.jdbc_account_link_backend)? else { return Ok(None) };
    let (accounts, codes) = table_names(&config.jdbc_table_prefix)?;
    let opts = OptsBuilder::new()
        .ip_or_hostname(Some(target.host.clone()))
        .tcp_port(target.port)
        .db_name(Some(target.database.clone()))
        .user((!config.jdbc_username.trim().is_empty()).then(|| config.jdbc_username.trim().to_string()))
        .pass((!config.jdbc_password.0.trim().is_empty()).then(|| config.jdbc_password.0.trim().to_string()));
    let mut conn = Conn::new(opts).map_err(|e| format!("JDBC connection failed: {e}"))?;
    conn.query_drop(create_accounts_sql(&accounts)).map_err(|e| format!("JDBC: {e}"))?;
    conn.query_drop(create_codes_sql(&codes)).map_err(|e| format!("JDBC: {e}"))?;
    Ok(Some(Box::new(MysqlStore { conn, accounts })))
}

pub struct MysqlStore {
    conn: Conn,
    accounts: String,
}

impl LinkStore for MysqlStore {
    fn read(&mut self) -> Result<Vec<(String, String)>, String> {
        self.conn.query(format!("select discord, uuid from {}", self.accounts)).map_err(|e| format!("JDBC: {e}"))
    }

    fn delete(&mut self, discord: &str) -> Result<(), String> {
        self.conn.exec_drop(format!("delete from {} where discord = ?", self.accounts), (discord,)).map_err(|e| format!("JDBC: {e}"))
    }

    fn upsert(&mut self, discord: &str, uuid: &str) -> Result<(), String> {
        self.conn.exec_drop(upsert_sql(&self.accounts), (discord, uuid)).map_err(|e| format!("JDBC: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_is_read_like_the_originals_pattern() {
        assert_eq!(
            parse_url("jdbc:mysql://db.example:3307/links?autoReconnect=true&useSSL=false").unwrap(),
            Some(Target { host: "db.example".into(), port: 3307, database: "links".into() })
        );
        assert_eq!(parse_url("jdbc:mysql://db.example/links").unwrap().map(|t| t.port), Some(3306));
    }

    #[test]
    fn the_placeholder_url_and_an_empty_one_mean_off() {
        assert_eq!(parse_url("jdbc:mysql://HOST:PORT/DATABASE?autoReconnect=true&useSSL=false").unwrap(), None);
        assert_eq!(parse_url("  ").unwrap(), None);
    }

    #[test]
    fn a_wrong_url_is_an_error() {
        assert!(parse_url("http://x:1/db").unwrap_err().contains("protocol"));
        assert!(parse_url("jdbc:postgresql://x:1/db").unwrap_err().contains("MySQL"));
        assert!(parse_url("jdbc:mysql://x:99999/db").is_err());
        assert!(parse_url("jdbc:mysql://x:1/d b").is_err());
        assert!(parse_url("jdbc:mysql://x:1").is_err());
    }

    #[test]
    fn table_names_follow_the_prefix() {
        assert_eq!(table_names("discordsrv").unwrap(), ("discordsrv_accounts".into(), "discordsrv_codes".into()));
        assert_eq!(table_names("").unwrap(), ("accounts".into(), "codes".into()));
        assert!(table_names("a;drop").is_err());
    }

    #[test]
    fn the_sql_has_the_originals_layout() {
        let sql = create_accounts_sql("p_accounts");
        assert!(sql.contains("discord varchar(32) not null") && sql.contains("uuid varchar(36) not null"));
        assert!(sql.contains("accounts_discord_uindex unique (discord)") && sql.contains("accounts_uuid_uindex unique (uuid)"));
        assert!(create_codes_sql("p_codes").contains("code char(4) not null primary key"));
        assert!(upsert_sql("t").contains("on duplicate key update"));
    }

    #[test]
    fn jdbc_is_off_without_a_url() {
        assert!(connect(&Config::default()).unwrap().is_none());
    }
}
