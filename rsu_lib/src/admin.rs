//! Interactive TCP admin interface for RSU nodes.
//!
//! Binds to `127.0.0.1:<port>` inside the node's network namespace:
//!
//! ```text
//! ip netns exec <rsu_ns> nc 127.0.0.1 9000
//! ```

use crate::control::Rsu;
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const BANNER: &[u8] = b"\
vigilant-parakeet RSU admin\r\n\
type 'help' for available commands\r\n\
\r\n";

const PROMPT: &[u8] = b"> ";

const HELP: &str = "\
commands:\r\n\
  info        node identity and server link\r\n\
  clients     known OBU clients\r\n\
  routes      VANET next-hop routing table\r\n\
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
    tracing::info!(addr = %bind_addr, "RSU admin interface bound");
    Ok(listener)
}

/// Spawn the accept loop for a pre-bound listener.  Call from an async context
/// (does not need to be inside the namespace — the socket is already bound).
pub fn spawn(rsu: Arc<Rsu>, std_listener: std::net::TcpListener) -> Result<()> {
    let listener = TcpListener::from_std(std_listener)?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let rsu = rsu.clone();
                    tracing::debug!(peer = %peer, "RSU admin connection accepted");
                    tokio::spawn(async move {
                        handle(rsu, stream, peer).await;
                    });
                }
                Err(e) => {
                    tracing::error!(error = %e, "RSU admin accept failed");
                }
            }
        }
    });
    Ok(())
}

async fn handle(rsu: Arc<Rsu>, stream: tokio::net::TcpStream, peer: SocketAddr) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    macro_rules! send {
        ($bytes:expr) => {
            if writer.write_all($bytes).await.is_err() {
                tracing::debug!(peer = %peer, "RSU admin connection closed");
                return;
            }
        };
    }

    send!(BANNER);
    send!(PROMPT);

    while let Ok(Some(raw)) = lines.next_line().await {
        let line = raw.trim();
        let (quit, response) = dispatch(&rsu, line);

        if !response.is_empty() {
            send!(response.as_bytes());
        }

        if quit {
            break;
        }

        send!(PROMPT);
    }

    tracing::debug!(peer = %peer, "RSU admin connection closed");
}

fn dispatch(rsu: &Rsu, line: &str) -> (bool, String) {
    let cmd = line
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    let response = match cmd.as_str() {
        "" => String::new(),

        "help" => HELP.to_string(),

        "info" => fmt_info(rsu),

        "clients" => fmt_clients(rsu),

        "routes" => fmt_routes(rsu),

        "quit" | "exit" => return (true, "bye\r\n".to_string()),

        other => format!("unknown command '{other}'\r\n"),
    };

    (false, response)
}

// ── Formatters ────────────────────────────────────────────────────────────────

fn fmt_info(rsu: &Rsu) -> String {
    let mut out = String::new();
    out.push_str(&format!("name     {}\r\n", rsu.node_name()));
    out.push_str(&format!("mac      {}\r\n", rsu.mac_address()));
    out.push_str(&format!("clients  {}\r\n", rsu.get_clients().len()));
    out.push_str(&format!("routes   {}\r\n", rsu.next_hop_count()));
    out
}

fn fmt_clients(rsu: &Rsu) -> String {
    let mut clients = rsu.get_clients();
    if clients.is_empty() {
        return "no OBU clients known\r\n".to_string();
    }
    clients.sort_by_key(|(obu, _)| obu.bytes());
    let mut out = format!("{:<19}  {}\r\n", "obu mac", "via mac");
    out.push_str(&"-".repeat(42));
    out.push_str("\r\n");
    for (obu, via) in clients {
        out.push_str(&format!("{obu:<19}  {via}\r\n"));
    }
    out
}

fn fmt_routes(rsu: &Rsu) -> String {
    let mut hops = rsu.get_next_hops_info();
    if hops.is_empty() {
        return "no VANET routes learned yet\r\n".to_string();
    }
    hops.sort_by_key(|(mac, _, _)| mac.bytes());
    let mut out = format!("{:<19}  {:>5}  {}\r\n", "next-hop mac", "hops", "latency");
    out.push_str(&"-".repeat(44));
    out.push_str("\r\n");
    for (mac, hops_count, lat_us) in hops {
        let lat_str = lat_us
            .map(|us| {
                if us < 1000 {
                    format!("{us}us")
                } else {
                    format!("{}ms", us / 1000)
                }
            })
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!("{mac:<19}  {hops_count:>5}  {lat_str}\r\n"));
    }
    out
}
