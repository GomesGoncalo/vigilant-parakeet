use anyhow::{bail, Result};
use clap::{Parser, ValueEnum};
use config::Config;
use node_lib::args::NodeType;
#[cfg(test)]
use node_lib::PACKET_BUFFER_SIZE;
#[cfg(test)]
#[allow(unused_imports)]
use std::{collections::HashMap, sync::Arc};
use tokio::signal;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod sim_args;
use sim_args::SimArgs;

mod channel;
mod interface_builder;
mod metrics;
mod namespace;
mod node_factory;
mod node_interfaces;
mod simulator;
mod topology;
#[cfg(feature = "tui")]
mod tui;
#[cfg(feature = "webview")]
mod webview;
use node_factory::create_node_from_settings;
use simulator::Simulator;

#[tokio::main]
async fn main() -> Result<()> {
    let args = SimArgs::parse();

    // Set up logging based on TUI mode
    #[cfg(feature = "tui")]
    let log_buffer = if args.tui {
        let buffer = tui::LogBuffer::new();
        let tui_layer = tui::TuiLogLayer::new(buffer.clone_buffer());

        tracing_subscriber::registry()
            .with(tui_layer)
            .with(EnvFilter::from_default_env())
            .init();

        Some(buffer)
    } else {
        if args.pretty {
            tracing_subscriber::registry()
                .with(fmt::layer().with_thread_ids(true).pretty())
                .with(EnvFilter::from_default_env())
                .init();
        } else {
            tracing_subscriber::registry()
                .with(fmt::layer().with_thread_ids(true))
                .with(EnvFilter::from_default_env())
                .init();
        }
        None
    };

    #[cfg(not(feature = "tui"))]
    {
        if args.pretty {
            tracing_subscriber::registry()
                .with(fmt::layer().with_thread_ids(true).pretty())
                .with(EnvFilter::from_default_env())
                .init();
        } else {
            tracing_subscriber::registry()
                .with(fmt::layer().with_thread_ids(true))
                .with(EnvFilter::from_default_env())
                .init();
        }
    }

    let simulator = std::sync::Arc::new(Simulator::new(&args, |_name, config| {
        let Some(config) = config.get("config_path") else {
            bail!("no config for node");
        };

        let config = config.to_string();

        let settings = Config::builder()
            .add_source(config::File::with_name(&config))
            .build()?;
        tracing::debug!(?settings, "Node configuration loaded");

        // Parse node type from config
        let node_type = NodeType::from_str(&settings.get_string("node_type")?, true)
            .map_err(|e| anyhow::anyhow!(e))?;

        // Create node with all its interfaces
        let result = create_node_from_settings(node_type, &settings)?;

        // Return complete NodeInterfaces (no more dummy tuns needed!)
        Ok((result.device, result.interfaces, result.node))
    })?);

    // Note: Server nodes are started synchronously in their namespace context during creation
    // via Server::start() in create_node_from_settings(), ensuring the socket binds within
    // the correct network namespace before returning

    // Spawn TUI if requested
    #[cfg(feature = "tui")]
    if args.tui {
        tracing::info!("Starting TUI dashboard...");
        let metrics = simulator.get_metrics();
        let log_buffer = log_buffer.unwrap().clone_buffer();

        // Start webview server if feature is enabled (before moving simulator)
        #[cfg(feature = "webview")]
        {
            tracing::info!("Starting webview API server on http://127.0.0.1:3030");
            let routes = webview::setup_routes(&simulator);
            let webview_handle = tokio::spawn(async move {
                warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
            });

            // Clone simulator for both TUI and simulator task
            let sim_for_task = simulator.clone();
            let tui_sim = simulator.clone();
            // Run simulator in background
            let sim_handle = tokio::spawn(async move {
                let _ = sim_for_task.run().await;
            });
            let tui_handle = tokio::spawn(async move {
                if let Err(e) = tui::run_tui(metrics, log_buffer, tui_sim).await {
                    tracing::error!("TUI error: {}", e);
                }
            });

            // Wait for either TUI, simulator, or webview to exit
            tokio::select! {
                _ = tui_handle => { tracing::info!("TUI exited"); }
                _ = sim_handle => { tracing::info!("Simulator exited"); }
                _ = webview_handle => { tracing::info!("Webview exited"); }
                _ = signal::ctrl_c() => { tracing::info!("Ctrl+C received"); }
            }
        }

        // No webview feature - just TUI and simulator
        #[cfg(not(feature = "webview"))]
        {
            let nodes = simulator.get_nodes();
            let tui_handle = tokio::spawn(async move {
                if let Err(e) = tui::run_tui(metrics, log_buffer, nodes).await {
                    tracing::error!("TUI error: {}", e);
                }
            });

            // Run simulator in background
            let sim_handle = tokio::spawn(async move {
                let _ = simulator.run().await;
            });

            tokio::select! {
                _ = tui_handle => { tracing::info!("TUI exited"); }
                _ = sim_handle => { tracing::info!("Simulator exited"); }
                _ = signal::ctrl_c() => { tracing::info!("Ctrl+C received"); }
            }
        }

        return Ok(());
    }

    // Spawn metrics printer task (prints every 30 seconds) if not using TUI
    let metrics = simulator.get_metrics();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let summary = metrics.summary();
            tracing::info!(
                packets_sent = summary.packets_sent,
                packets_dropped = summary.packets_dropped,
                drop_rate = format!("{:.2}%", summary.drop_rate),
                avg_latency = format!("{:.2}ms", summary.avg_latency_ms()),
                throughput = format!("{:.2} pkt/s", summary.packets_per_second()),
                uptime = format!("{:.1}s", summary.uptime.as_secs_f64()),
                "Simulation metrics"
            );
        }
    });

    #[cfg(feature = "webview")]
    {
        let routes = webview::setup_routes(&simulator);
        tokio::select! {
            _ = warp::serve(routes).run(([127, 0, 0, 1], 3030)) => {}
            _ = simulator.run() => {}
            _ = signal::ctrl_c() => {}
        }
    }
    #[cfg(not(feature = "webview"))]
    {
        tokio::select! {
            _ = simulator.run() => {}
            _ = signal::ctrl_c() => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::channel_parameters::ChannelParameters;
    use mac_address::MacAddress;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn channel_set_params_updates_and_allows_send() {
        // Create a dummy tun from test helpers
        let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(tun_a);
        let params = ChannelParameters::from(std::collections::HashMap::new());
        let mac = MacAddress::new([0, 1, 2, 3, 4, 5]);

        // Channel::new spawns a background task; use a small topology-style from/to names
        let ch = crate::channel::Channel::new(
            params,
            mac,
            tun.clone(),
            &"from".to_string(),
            &"to".to_string(),
        );

        // Setting params via set_params should accept a valid map
        let mut map = HashMap::new();
        map.insert("latency".to_string(), "0".to_string());
        map.insert("loss".to_string(), "0".to_string());

        assert!(ch.set_params(map).is_ok());

        // Now exercise send/should_send by sending a packet with the correct MAC
        let mut packet = [0u8; PACKET_BUFFER_SIZE];
        // destination mac = our mac
        packet[0..6].copy_from_slice(&mac.bytes());
        // payload small
        packet[6] = 0x42;

        let res = ch.send(packet, 7).await;
        assert!(res.is_ok());
    }

    #[cfg(feature = "webview")]
    #[test]
    fn error_message_serialize() {
        let em = crate::webview::ErrorMessage {
            code: 404,
            message: "not found".to_string(),
        };

        let v = serde_json::to_value(&em).expect("serialize");
        assert_eq!(v["code"].as_i64().unwrap(), 404);
        assert_eq!(v["message"].as_str().unwrap(), "not found");
    }
}
