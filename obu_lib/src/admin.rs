//! Interactive TCP admin interface for OBU nodes.
//!
//! Binds to `127.0.0.1:<port>` inside the node's network namespace:
//!
//! ```text
//! ip netns exec <obu_ns> nc 127.0.0.1 9000
//! ```

use crate::control::Obu;
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const BANNER: &[u8] = b"\
vigilant-parakeet OBU admin\r\n\
type 'help' for available commands\r\n\
\r\n";

const PROMPT: &[u8] = b"> ";

const HELP: &str = "\
commands:\r\n\
  info        node identity and upstream route\r\n\
  session     DH session status with the server\r\n\
  routes      cached upstream candidates\r\n\
  rekey       clear current session and trigger immediate re-key\r\n\
  help        show this help\r\n\
  quit        close connection\r\n\
\r\n";

/// Bind a TCP admin listener synchronously (using the calling thread's network
/// namespace) and return it so the caller can later call [`spawn`].
///
/// Call this while the thread is inside the target namespace.
pub fn bind(bind_addr: SocketAddr) -> Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(bind_addr)?;
    listener.set_nonblocking(true)?;
    tracing::info!(addr = %bind_addr, "OBU admin interface bound");
    Ok(listener)
}

/// Spawn the accept loop for a pre-bound listener.  Call from an async context
/// (does not need to be inside the namespace — the socket is already bound).
pub fn spawn(obu: Arc<Obu>, std_listener: std::net::TcpListener) -> Result<()> {
    let listener = TcpListener::from_std(std_listener)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let obu = obu.clone();
                    tracing::debug!(peer = %peer, "OBU admin connection accepted");
                    tokio::spawn(async move {
                        handle(obu, stream, peer).await;
                    });
                }
                Err(e) => {
                    tracing::error!(error = %e, "OBU admin accept failed");
                }
            }
        }
    });
    Ok(())
}

async fn handle(obu: Arc<Obu>, stream: tokio::net::TcpStream, peer: SocketAddr) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    macro_rules! send {
        ($bytes:expr) => {
            if writer.write_all($bytes).await.is_err() {
                tracing::debug!(peer = %peer, "OBU admin connection closed");
                return;
            }
        };
    }

    send!(BANNER);
    send!(PROMPT);

    while let Ok(Some(raw)) = lines.next_line().await {
        let line = raw.trim();
        let (quit, response) = dispatch(&obu, line);

        if !response.is_empty() {
            send!(response.as_bytes());
        }

        if quit {
            break;
        }

        send!(PROMPT);
    }

    tracing::debug!(peer = %peer, "OBU admin connection closed");
}

fn dispatch(obu: &Obu, line: &str) -> (bool, String) {
    let cmd = line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    let response = match cmd.as_str() {
        "" => String::new(),

        "help" => HELP.to_string(),

        "info" => fmt_info(obu),

        "session" => fmt_session(obu),

        "routes" => fmt_routes(obu),

        "rekey" => {
            obu.trigger_rekey();
            "DH session cleared — re-key exchange initiated\r\n".to_string()
        }

        "quit" | "exit" => return (true, "bye\r\n".to_string()),

        other => format!("unknown command '{other}'\r\n"),
    };

    (false, response)
}

// ── Formatters ────────────────────────────────────────────────────────────────

fn fmt_info(obu: &Obu) -> String {
    let mut out = String::new();
    out.push_str(&format!("name    {}\r\n", obu.node_name()));
    out.push_str(&format!("mac     {}\r\n", obu.mac_address()));
    match obu.cached_upstream_mac() {
        Some(mac) => out.push_str(&format!("upstream  {mac}\r\n")),
        None => out.push_str("upstream  (none)\r\n"),
    }
    out.push_str(&format!(
        "session   {}\r\n",
        if obu.has_dh_session() {
            "established"
        } else {
            "none"
        }
    ));
    out
}

fn fmt_session(obu: &Obu) -> String {
    match obu.get_dh_session_info() {
        Some((key_id, age_secs)) => {
            let pending = if obu.has_dh_pending() {
                " (rekey pending)"
            } else {
                ""
            };
            format!(
                "key_id  {key_id}\r\nage     {}{pending}\r\n",
                fmt_age(age_secs)
            )
        }
        None => {
            if obu.has_dh_pending() {
                "no established session — key exchange in progress\r\n".to_string()
            } else {
                "no DH session\r\n".to_string()
            }
        }
    }
}

fn fmt_routes(obu: &Obu) -> String {
    let candidates = obu.get_upstream_candidates();
    if candidates.is_empty() {
        return "no upstream candidates\r\n".to_string();
    }
    let mut out = format!("{:<5}  {}\r\n", "rank", "upstream mac");
    out.push_str(&"-".repeat(28));
    out.push_str("\r\n");
    for (i, mac) in candidates.iter().enumerate() {
        let rank = if i == 0 {
            "prim".to_string()
        } else {
            format!("fb{i}")
        };
        out.push_str(&format!("{rank:<5}  {mac}\r\n"));
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
