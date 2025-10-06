//! Event handling for the TUI

use crate::tui::logging::LogFilter;
use crate::tui::state::{DisplayChannelStats, Tab, TuiState};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

/// Handle a keyboard event
/// Returns Ok(true) if the application should quit, Ok(false) to continue
pub fn handle_key_event(key: KeyEvent, state: &mut TuiState) -> Result<bool> {
    match key.code {
        // Quit commands
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            tracing::info!("Quit command received");
            return Ok(true);
        }
        KeyCode::Esc => {
            // Exit input mode if active, otherwise quit
            if state.log_input_mode {
                state.log_input_mode = false;
                state.log_input_buffer.clear();
            } else {
                tracing::info!("Escape key received");
                return Ok(true);
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            tracing::info!("Ctrl+C received");
            return Ok(true);
        }

        // Tab switching
        KeyCode::Tab | KeyCode::BackTab => {
            if !state.log_input_mode {
                state.active_tab = match state.active_tab {
                    Tab::Metrics => Tab::Channels,
                    Tab::Channels => Tab::Upstreams,
                    Tab::Upstreams => Tab::Logs,
                    Tab::Logs => Tab::Topology,
                    Tab::Topology => Tab::Metrics,
                };
            }
        }
        KeyCode::Char('1') if !state.log_input_mode => {
            state.active_tab = Tab::Metrics;
        }
        KeyCode::Char('2') if !state.log_input_mode => {
            state.active_tab = Tab::Channels;
        }
        KeyCode::Char('3') if !state.log_input_mode => {
            state.active_tab = Tab::Upstreams;
        }
        KeyCode::Char('4') if !state.log_input_mode => {
            state.active_tab = Tab::Logs;
        }
        KeyCode::Char('5') if !state.log_input_mode => {
            state.active_tab = Tab::Topology;
        }

        // Reset metrics
        KeyCode::Char('r') | KeyCode::Char('R') if !state.log_input_mode => {
            state.metrics.reset();
            state.packets_sent_history.clear();
            state.loss_percentage_history.clear();
            state.throughput_history.clear();
            state.latency_history.clear();
            state.p95_history.clear();
            state.p99_history.clear();
            state.throughput_bps_history.clear();
            state.prev_packets_sent = 0;
            state.prev_packets_dropped = 0;
            state.prev_timestamp = 0.0;
            state.start_time = Instant::now();
        }

        // Channel sort mode (Channels tab only)
        KeyCode::Char('s') | KeyCode::Char('S') if !state.log_input_mode => {
            if state.active_tab == Tab::Channels {
                state.channel_sort_mode = state.channel_sort_mode.next();
            }
        }

        // Pause/unpause
        KeyCode::Char('p') | KeyCode::Char('P') if !state.log_input_mode => {
            handle_pause_toggle(state);
        }

        // Sort direction toggle (Channels tab only)
        KeyCode::Char('d') | KeyCode::Char('D') if !state.log_input_mode => {
            if state.active_tab == Tab::Channels {
                state.channel_sort_direction = state.channel_sort_direction.toggle();
            }
        }

        // Log filter cycling (Logs tab only)
        KeyCode::Char('f') | KeyCode::Char('F') if !state.log_input_mode => {
            if state.active_tab == Tab::Logs {
                state.log_filter = state.log_filter.next();
                state.log_scroll = 0;
                state.log_auto_scroll = true;
            }
        }

        // Enter custom filter mode (Logs tab only)
        KeyCode::Char('/') if !state.log_input_mode => {
            if state.active_tab == Tab::Logs {
                state.log_input_mode = true;
                state.log_input_buffer.clear();
            }
        }

        // Input mode: add character
        KeyCode::Char(c) if state.log_input_mode => {
            state.log_input_buffer.push(c);
        }

        // Input mode: backspace
        KeyCode::Backspace if state.log_input_mode => {
            state.log_input_buffer.pop();
        }

        // Input mode: apply filter
        KeyCode::Enter if state.log_input_mode => {
            if state.log_input_buffer.is_empty() {
                state.log_filter = LogFilter::All;
            } else {
                state.log_filter = LogFilter::Custom(state.log_input_buffer.clone());
            }
            state.log_input_mode = false;
            state.log_scroll = 0;
            state.log_auto_scroll = true;
        }

        // Navigation: Up
        KeyCode::Up => {
            if state.active_tab == Tab::Logs {
                if state.log_scroll > 0 {
                    state.log_scroll -= 1;
                }
                state.log_auto_scroll = false;
            } else if state.active_tab == Tab::Topology
                && state.selected_topology_index > 0
            {
                state.selected_topology_index -= 1;
            }
        }

        // Navigation: Down
        KeyCode::Down => {
            if state.active_tab == Tab::Logs {
                state.log_scroll += 1;
                state.log_auto_scroll = false;
            } else if state.active_tab == Tab::Topology
                && state.selected_topology_index + 1 < state.topology_item_count
            {
                state.selected_topology_index += 1;
            }
        }

        // Navigation: Page Up
        KeyCode::PageUp => {
            if state.active_tab == Tab::Logs {
                state.log_scroll = state.log_scroll.saturating_sub(10);
                state.log_auto_scroll = false;
            } else if state.active_tab == Tab::Topology {
                state.selected_topology_index = state.selected_topology_index.saturating_sub(5);
            }
        }

        // Navigation: Page Down
        KeyCode::PageDown => {
            if state.active_tab == Tab::Logs {
                state.log_scroll = state.log_scroll.saturating_add(10);
                state.log_auto_scroll = false;
            } else if state.active_tab == Tab::Topology {
                state.selected_topology_index = (state.selected_topology_index + 5)
                    .min(state.topology_item_count.saturating_sub(1));
            }
        }

        // Navigation: Home
        KeyCode::Home => {
            if state.active_tab == Tab::Logs {
                state.log_scroll = 0;
                state.log_auto_scroll = false;
            } else if state.active_tab == Tab::Topology {
                state.selected_topology_index = 0;
            }
        }

        // Navigation: End
        KeyCode::End => {
            if state.active_tab == Tab::Logs {
                let log_count = state.log_buffer.lock().unwrap().len();
                state.log_scroll = log_count.saturating_sub(1);
                state.log_auto_scroll = true;
            } else if state.active_tab == Tab::Topology && state.topology_item_count > 0 {
                state.selected_topology_index = state.topology_item_count - 1;
            }
        }

        _ => {}
    }

    Ok(false) // Continue running
}

/// Handle pause/unpause toggle
fn handle_pause_toggle(state: &mut TuiState) {
    state.paused = !state.paused;

    if state.paused {
        // Capture snapshots
        state.paused_summary = Some(state.metrics.summary());

        // Compute concrete display snapshot for channels
        let live = state.metrics.channel_stats();
        let mut display_map = std::collections::HashMap::new();
        for (k, v) in live.into_iter() {
            let throughput_bps = v.throughput_last(10) * 8.0;
            let avg_latency_ms = if v.packets_delayed > 0 {
                (v.total_latency_us as f64 / v.packets_delayed as f64) / 1000.0
            } else {
                0.0
            };
            display_map.insert(
                k.clone(),
                DisplayChannelStats {
                    packets_sent: v.packets_sent,
                    packets_dropped: v.packets_dropped,
                    throughput_bps,
                    avg_latency_ms,
                },
            );
        }
        state.paused_channel_display = Some(display_map);

        // Capture upstreams snapshot
        capture_upstreams_snapshot(state);
    } else {
        // Clear snapshots and enable auto-scroll
        state.paused_summary = None;
        state.paused_channel_display = None;
        state.log_auto_scroll = true;
        state.paused_upstreams = None;
    }
}

/// Capture upstream routing information snapshot
fn capture_upstreams_snapshot(state: &mut TuiState) {
    let mut ups: Vec<(String, String, String, String, String, String)> = Vec::new();

    for (name, (mac, ntype, simnode)) in state.nodes.iter() {
        if ntype != "Obu" {
            continue;
        }

        let obu_mac = mac.clone();
        let mut upstream_display = "(no upstream)".to_string();
        let mut upstream_mac = "-".to_string();
        let mut hops = "-".to_string();
        let mut next_hop = "-".to_string();

        if let crate::simulator::SimNode::Obu(ref o) = simnode {
            if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                if let Some(route) = obu.cached_upstream_route() {
                    upstream_mac = format!("{}", route.mac);
                    next_hop = upstream_mac.clone();

                    // Attempt to resolve final RSU name and total hops
                    let mut total_hops = route.hops;
                    let mut current_mac = format!("{}", route.mac);
                    let mut depth = 0;

                    let final_name = loop {
                        if depth > 16 {
                            break None;
                        }
                        depth += 1;

                        if let Some((nname, (_m, ntype2, snode))) =
                            state.nodes.iter().find(|(_, (m, _, _))| **m == current_mac)
                        {
                            if ntype2 == "Rsu" {
                                break Some(nname.clone());
                            }
                            if ntype2 == "Obu" {
                                if let crate::simulator::SimNode::Obu(ref other_o) = snode {
                                    if let Some(other_obu) =
                                        other_o.as_any().downcast_ref::<obu_lib::Obu>()
                                    {
                                        if let Some(next_route) = other_obu.cached_upstream_route()
                                        {
                                            total_hops = total_hops.saturating_add(next_route.hops);
                                            current_mac = format!("{}", next_route.mac);
                                            continue;
                                        }
                                    }
                                }
                                break None;
                            }
                            break None;
                        } else {
                            break None;
                        }
                    };

                    hops = final_name
                        .as_ref()
                        .map(|_| format!("{}", total_hops))
                        .unwrap_or_else(|| format!("{}", route.hops));
                    upstream_display = final_name.unwrap_or_else(|| format!("{}", route.mac));
                }
            }
        }

        ups.push((
            name.clone(),
            obu_mac.clone(),
            upstream_display,
            upstream_mac,
            hops,
            next_hop,
        ));
    }

    state.paused_upstreams = Some(ups);
}
