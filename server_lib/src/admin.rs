//! Interactive TCP admin interface for the server node.
//!
//! Bind address is inside the network namespace (typically 127.0.0.1:<port>),
//! so access requires entering the namespace first:
//!
//! ```text
//! ip netns exec <server_ns> nc 127.0.0.1 9000
//! ip netns exec <server_ns> telnet 127.0.0.1 9000
//! ```

use crate::server::Server;
use anyhow::Result;
use mac_address::MacAddress;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const BANNER: &[u8] = b"\
vigilant-parakeet server admin\r\n\
type 'help' for available commands\r\n\
\r\n";

const PROMPT: &[u8] = b"> ";

const HELP: &str = "\
commands:\r\n\
  sessions              list active DH sessions\r\n\
  revoke <mac>          terminate session (e.g. revoke aa:bb:cc:dd:ee:ff)\r\n\
  routes                show OBU routing table\r\n\
  registry              show RSU -> OBU associations\r\n\
  allowlist             show PKI signing allowlist\r\n\
  help                  show this help\r\n\
  quit                  close connection\r\n\
\r\n";

/// Bind a TCP admin listener synchronously (using the calling thread's network
/// namespace) and return it so the caller can later call [`spawn`].
///
/// Call this while the thread is inside the target namespace.
pub fn bind(bind_addr: SocketAddr) -> Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(bind_addr)?;
    listener.set_nonblocking(true)?;
    tracing::info!(addr = %bind_addr, "Server admin interface bound (use nc or telnet to connect)");
    Ok(listener)
}

/// Spawn the accept loop for a pre-bound listener.  Call from an async context
/// (does not need to be inside the namespace — the socket is already bound).
pub fn spawn(server: Arc<Server>, std_listener: std::net::TcpListener) -> Result<()> {
    let listener = TcpListener::from_std(std_listener)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let server = server.clone();
                    tracing::debug!(peer = %peer, "Admin connection accepted");
                    tokio::spawn(async move {
                        handle(server, stream, peer).await;
                    });
                }
                Err(e) => {
                    tracing::error!(error = %e, "Admin accept failed");
                }
            }
        }
    });
    Ok(())
}

async fn handle(server: Arc<Server>, stream: tokio::net::TcpStream, peer: SocketAddr) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    macro_rules! send {
        ($bytes:expr) => {
            if writer.write_all($bytes).await.is_err() {
                tracing::debug!(peer = %peer, "Admin connection closed");
                return;
            }
        };
    }

    send!(BANNER);
    send!(PROMPT);

    while let Ok(Some(raw)) = lines.next_line().await {
        let line = raw.trim();

        let (quit, response) = dispatch(&server, line).await;

        if !response.is_empty() {
            send!(response.as_bytes());
        }

        if quit {
            break;
        }

        send!(PROMPT);
    }

    tracing::debug!(peer = %peer, "Admin connection closed");
}

/// Returns `(quit, response_text)`.
async fn dispatch(server: &Server, line: &str) -> (bool, String) {
    let mut iter = line.splitn(2, ' ');
    let cmd = iter.next().unwrap_or("").to_ascii_lowercase();
    let arg = iter.next().map(str::trim);

    let response = match cmd.as_str() {
        "" => String::new(),

        "help" => HELP.to_string(),

        "sessions" => fmt_sessions(server).await,

        "revoke" => match arg {
            None => "usage: revoke <mac>\r\n".to_string(),
            Some(mac_str) => match parse_mac(mac_str) {
                None => format!("error: invalid MAC address '{mac_str}'\r\n"),
                Some(mac) => {
                    let had = server.revoke_node(mac).await;
                    if had {
                        format!("session for {mac} revoked\r\n")
                    } else {
                        format!("no active session for {mac} (server-side key cleared; notification sent if route is known)\r\n")
                    }
                }
            },
        },

        "routes" => fmt_routes(server).await,

        "registry" => fmt_registry(server).await,

        "allowlist" => fmt_allowlist(server).await,

        "quit" | "exit" => return (true, "bye\r\n".to_string()),

        other => format!("unknown command '{other}'\r\n"),
    };

    (false, response)
}

// ── Formatters ────────────────────────────────────────────────────────────────

async fn fmt_sessions(server: &Server) -> String {
    let sessions = server.get_sessions().await;
    if sessions.is_empty() {
        return "no active sessions\r\n".to_string();
    }
    let mut out = format!("{:<19}  {:>7}  {:>8}\r\n", "obu vanet mac", "key_id", "age");
    out.push_str(&"-".repeat(42));
    out.push_str("\r\n");
    for (mac, key_id, age_secs) in sessions {
        out.push_str(&format!(
            "{mac:<19}  {key_id:>7}  {:>8}\r\n",
            fmt_age(age_secs)
        ));
    }
    out
}

async fn fmt_routes(server: &Server) -> String {
    let routes = server.get_routes().await;
    if routes.is_empty() {
        return "no OBU routes learned yet\r\n".to_string();
    }
    let mut out = format!("{:<19}  {:<19}  {}\r\n", "tap mac", "vanet mac", "via rsu");
    out.push_str(&"-".repeat(60));
    out.push_str("\r\n");
    for (tap_mac, vanet_mac, rsu_addr) in routes {
        out.push_str(&format!("{tap_mac:<19}  {vanet_mac:<19}  {rsu_addr}\r\n"));
    }
    out
}

async fn fmt_registry(server: &Server) -> String {
    let registry = server.get_registry().await;
    if registry.is_empty() {
        return "no RSUs registered yet\r\n".to_string();
    }
    let mut out = String::new();
    for (rsu, obus) in &registry {
        out.push_str(&format!("rsu {rsu}  ({} obu(s))\r\n", obus.len()));
        for obu in obus {
            out.push_str(&format!("    {obu}\r\n"));
        }
    }
    out
}

async fn fmt_allowlist(server: &Server) -> String {
    let list = server.get_dh_signing_allowlist().await;
    if list.is_empty() {
        return "allowlist empty — PKI check disabled, all OBUs may exchange keys\r\n".to_string();
    }
    let mut out = format!("{:<19}  {}\r\n", "obu vanet mac", "verifying key (hex)");
    out.push_str(&"-".repeat(84));
    out.push_str("\r\n");
    for (mac, bytes) in &list {
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&format!("{mac:<19}  {hex}\r\n"));
    }
    out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fmt_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Parse a colon-separated hex MAC address (e.g. `aa:bb:cc:dd:ee:ff`).
fn parse_mac(s: &str) -> Option<MacAddress> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut bytes = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        bytes[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(MacAddress::new(bytes))
}
