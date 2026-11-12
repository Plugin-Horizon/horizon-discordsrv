//! A websocket client connection: TCP (and TLS through rustls for `wss://`), the HTTP upgrade
//! of RFC 6455 written here (Horizon's tungstenite has no handshake feature), then tungstenite's
//! protocol over the stream.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use sha1::{Digest, Sha1};
use tungstenite::WebSocket;
use tungstenite::protocol::Role;

pub enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// `ws://` or `wss://` URL parts.
#[derive(Debug, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, at least `/`.
    pub resource: String,
}

pub fn parse_url(url: &str) -> Result<Url, String> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("ws://") {
        (false, r)
    } else {
        return Err(format!("not a websocket URL: {url}"));
    };
    let (authority, resource) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("bad port in {url}"))?),
        None => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("no host in {url}"));
    }
    Ok(Url { tls, host: host.to_string(), port, resource })
}

/// `Sec-WebSocket-Accept` for a key (RFC 6455 section 4.2.2).
pub fn accept_key(key: &str) -> String {
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha.finalize())
}

/// A fresh 16-byte nonce for `Sec-WebSocket-Key` (not a secret: only unique per connection).
fn nonce() -> String {
    let mut sha = Sha1::new();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    sha.update(now.as_nanos().to_le_bytes());
    sha.update(std::process::id().to_le_bytes());
    sha.update(format!("{:p}", &now).as_bytes());
    base64::engine::general_purpose::STANDARD.encode(&sha.finalize()[..16])
}

/// A connected websocket, and the TCP socket to set its read timeout on.
pub struct Connection {
    pub socket: WebSocket<Stream>,
    pub tcp: TcpStream,
}

pub fn connect(url: &str) -> Result<Connection, String> {
    let url = parse_url(url)?;
    let addr = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {}: {e}", url.host))?
        .next()
        .ok_or_else(|| format!("no address for {}", url.host))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| format!("connect: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    let control = tcp.try_clone().map_err(|e| e.to_string())?;
    let mut stream = if url.tls {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(url.host.clone()).map_err(|e| e.to_string())?;
        let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
        Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
    } else {
        Stream::Plain(tcp)
    };
    let key = nonce();
    let host = if url.port == if url.tls { 443 } else { 80 } { url.host.clone() } else { format!("{}:{}", url.host, url.port) };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        url.resource
    );
    stream.write_all(request.as_bytes()).and_then(|()| stream.flush()).map_err(|e| format!("handshake: {e}"))?;
    let head = read_head(&mut stream)?;
    check_upgrade(&head, &key)?;
    let socket = WebSocket::from_raw_socket(stream, Role::Client, None);
    Ok(Connection { socket, tcp: control })
}

/// The response head, read a byte at a time so nothing of the first frame is taken with it.
fn read_head(stream: &mut Stream) -> Result<String, String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return Err("handshake: response head too long".into());
        }
        match stream.read(&mut byte) {
            Ok(0) => return Err("handshake: connection closed".into()),
            Ok(_) => head.push(byte[0]),
            Err(e) => return Err(format!("handshake: {e}")),
        }
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

pub fn check_upgrade(head: &str, key: &str) -> Result<(), String> {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("101") {
        return Err(format!("handshake: {status}"));
    }
    let accept = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept"))
        .map(|(_, v)| v.trim().to_string());
    if accept.as_deref() != Some(accept_key(key).as_str()) {
        return Err("handshake: wrong Sec-WebSocket-Accept".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6455_accept_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn urls_split_into_parts() {
        assert_eq!(
            parse_url("wss://gateway.discord.gg/?v=10&encoding=json").unwrap(),
            Url { tls: true, host: "gateway.discord.gg".into(), port: 443, resource: "/?v=10&encoding=json".into() }
        );
        assert_eq!(
            parse_url("ws://127.0.0.1:9000?v=10").unwrap(),
            Url { tls: false, host: "127.0.0.1".into(), port: 9000, resource: "/?v=10".into() }
        );
        assert!(parse_url("https://x").is_err());
    }

    #[test]
    fn upgrade_answers_are_checked() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let ok = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
        assert!(check_upgrade(ok, key).is_ok());
        assert!(check_upgrade(&ok.replace("s3pP", "xxxx"), key).is_err());
        assert!(check_upgrade("HTTP/1.1 400 Bad Request\r\n\r\n", key).is_err());
    }

    #[test]
    fn nonces_are_16_bytes() {
        let n = base64::engine::general_purpose::STANDARD.decode(nonce()).unwrap();
        assert_eq!(n.len(), 16);
    }
}
