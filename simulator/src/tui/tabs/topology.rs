// Topology tab rendering - network tree visualization
use crate::{metrics::SimulatorMetrics, tui::state::TuiState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use super::TabRenderer;

/// State data for the Topology tab
pub struct TopologyTabState<'a> {
    pub nodes: &'a HashMap<String, (String, String, crate::simulator::SimNode)>,
    pub metrics: &'a Arc<SimulatorMetrics>,
    pub selected_index: &'a mut usize,
    pub item_count: &'a mut usize,
}

/// Topology tab renderer
#[derive(Default)]
pub struct TopologyTab;

impl TabRenderer for TopologyTab {
    type State<'a> = TopologyTabState<'a>;

    fn display_name() -> &'static str {
        "🌳 Topology"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        TopologyTabState {
            nodes: &tui_state.nodes,
            metrics: &tui_state.metrics,
            selected_index: &mut tui_state.selected_topology_index,
            item_count: &mut tui_state.topology_item_count,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        // Help text doesn't mutate these fields
        static mut DUMMY_INDEX: usize = 0;
        static mut DUMMY_COUNT: usize = 0;
        #[allow(static_mut_refs)]
        TopologyTabState {
            nodes: &tui_state.nodes,
            metrics: &tui_state.metrics,
            selected_index: unsafe { &mut DUMMY_INDEX },
            item_count: unsafe { &mut DUMMY_COUNT },
        }
    }

    fn render(f: &mut Frame, area: Rect, mut state: Self::State<'_>) {
        render_topology_tab(f, area, &mut state);
    }

    fn help_text(_state: Self::State<'_>) -> Vec<Span<'static>> {
        vec![
            super::key_span("Q/Esc/Ctrl+C"),
            super::text_span(" quit  │  "),
            super::key_span("P"),
            super::text_span(" pause  │  "),
            super::key_span("↑/↓/PgUp/PgDn"),
            super::text_span(" navigate  │  "),
            super::key_span("Tab/1/2/3/4/5"),
            super::text_span(" switch tabs"),
        ]
    }
}

/// Render the topology tab showing the network tree structure
pub fn render_topology_tab(f: &mut Frame, area: Rect, state: &mut TopologyTabState) {
    // Map mac -> node name for quick parent lookup
    let mut name_by_mac: HashMap<String, String> = HashMap::new();
    for (name, (mac, _ntype, _snode)) in state.nodes.iter() {
        name_by_mac.insert(mac.clone(), name.clone());
    }

    // children: parent_name -> Vec<child_name>
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // parent map for quick root detection
    let mut parent_map: HashMap<String, String> = HashMap::new();

    // Ensure every non-server node has an entry
    let mut server_names: HashSet<String> = HashSet::new();
    for (name, (_mac, ntype, _)) in state.nodes.iter() {
        if ntype == "Server" {
            server_names.insert(name.clone());
            continue;
        }
        children.entry(name.clone()).or_default();
    }

    let mut unattached: Vec<String> = Vec::new();

    // For each OBU, attempt to attach it to its immediate upstream node (by route.mac)
    for (name, (_mac, ntype, simnode)) in state.nodes.iter() {
        if ntype != "Obu" {
            continue;
        }

        let mut attached_parent: Option<String> = None;
        if let crate::simulator::SimNode::Obu(ref o) = simnode {
            if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                if let Some(route) = obu.cached_upstream_route() {
                    let immediate_mac = format!("{}", route.mac);
                    if let Some(parent_name) = name_by_mac.get(&immediate_mac) {
                        // skip servers as parents
                        if !server_names.contains(parent_name) {
                            attached_parent = Some(parent_name.clone());
                        }
                    }
                }
            }
        }

        if let Some(p) = attached_parent {
            children.entry(p.clone()).or_default().push(name.clone());
            parent_map.insert(name.clone(), p);
        } else {
            unattached.push(name.clone());
        }
    }

    // Prepare sorted roots: prefer RSUs first, then other roots (unattached nodes)
    let mut rsus: Vec<String> = state
        .nodes
        .iter()
        .filter_map(|(n, (_mac, ntype, _))| {
            if ntype == "Rsu" {
                Some(n.clone())
            } else {
                None
            }
        })
        .collect();
    rsus.sort();

    // Roots that are not RSUs (and have no parent) - omit servers
    let mut other_roots: Vec<String> = state
        .nodes
        .iter()
        .filter_map(|(n, (_mac, ntype, _))| {
            if ntype == "Server" {
                None
            } else if !parent_map.contains_key(n) && !rsus.contains(n) {
                Some(n.clone())
            } else {
                None
            }
        })
        .collect();
    other_roots.sort();

    // Build a flattened list of (display_line, node_name) in the order the tree would be printed
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut visited = HashSet::new();
    let mut memo: HashMap<String, usize> = HashMap::new();

    let mut ctx = TreeContext {
        state,
        children: &children,
        name_by_mac: &name_by_mac,
        visited: &mut visited,
        memo: &mut memo,
    };

    // Collect for RSU roots first
    for (i, rsu) in rsus.iter().enumerate() {
        let is_last = i + 1 == rsus.len() && other_roots.is_empty();
        collect_node(rsu, &mut ctx, &mut lines, "", is_last);
        if i + 1 != rsus.len() || !other_roots.is_empty() {
            lines.push((String::new(), String::new()));
        }
    }

    // Then other roots
    for (i, root) in other_roots.iter().enumerate() {
        let is_last = i + 1 == other_roots.len();
        collect_node(root, &mut ctx, &mut lines, "", is_last);
        if i + 1 != other_roots.len() {
            lines.push((String::new(), String::new()));
        }
    }

    // Add unattached OBUs section if present
    let mut unattached_display: Vec<String> = Vec::new();
    for name in unattached.iter() {
        if !parent_map.contains_key(name) && !rsus.contains(name) {
            if let Some((mac, _ntype, _snode)) = state.nodes.get(name) {
                unattached_display.push(format!("{} ({})", name, mac));
            } else {
                unattached_display.push(name.clone());
            }
        }
    }
    if !unattached_display.is_empty() {
        lines.push((String::new(), String::new()));
        lines.push(("Unattached OBUs:".to_string(), String::new()));
        for item in unattached_display.into_iter() {
            lines.push((format!("├─ {}", item), String::new()));
        }
    }

    // Update topology state with flattened count
    *state.item_count = lines.len();

    // Build List items and render with selection
    let items: Vec<ListItem> = lines
        .iter()
        .map(|(text, _name)| ListItem::new(text.clone()))
        .collect();

    let mut list_state = ListState::default();
    if *state.item_count > 0 && *state.selected_index < *state.item_count {
        list_state.select(Some(*state.selected_index));
    } else {
        list_state.select(None);
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Topology"))
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, chunks[0], &mut list_state);

    // If something is selected and corresponds to a node name, show details in right pane
    if let Some(idx) = list_state.selected() {
        if idx < lines.len() {
            let node_name = &lines[idx].1;
            if !node_name.is_empty() {
                render_node_details(f, chunks[1], state, node_name, &name_by_mac);
            }
        }
    }
}

/// Render detailed statistics for a selected node
fn render_node_details(
    f: &mut Frame,
    area: Rect,
    state: &TopologyTabState,
    node_name: &str,
    name_by_mac: &HashMap<String, String>,
) {
    // Compute end-to-end latency and packet loss to upstream RSU (if available)
    let channel_map = state.metrics.channel_stats();

    // Walk path from node to RSU and collect ordered segments (from -> to)
    let mut segments: Vec<(String, String)> = Vec::new();
    let mut current = node_name.to_string();
    let mut first_hop_from: Option<(String, String)> = None; // (from, to)
    let mut seen_path: HashSet<String> = HashSet::new();
    let mut reached_rsu = false;
    while !seen_path.contains(&current) {
        seen_path.insert(current.clone());
        // get this node's immediate upstream via cached_upstream_route if OBU
        if let Some((_mac, ntype, snode)) = state.nodes.get(&current) {
            if ntype == "Rsu" {
                reached_rsu = true;
                break;
            }
            if ntype != "Obu" {
                break;
            }
            if let crate::simulator::SimNode::Obu(ref o) = snode {
                if let Some(obu_ref) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                    if let Some(route) = obu_ref.cached_upstream_route() {
                        let next_mac = format!("{}", route.mac);
                        if let Some(next_name) = name_by_mac.get(&next_mac) {
                            // record segment current -> next_name
                            if first_hop_from.is_none() {
                                first_hop_from = Some((current.clone(), next_name.clone()));
                            }
                            segments.push((current.clone(), next_name.clone()));
                            current = next_name.clone();
                            continue;
                        }
                    }
                }
            }
        }
        break;
    }

    // Aggregate forward (selected -> RSU) stats across all segments
    let mut f_total_latency_us: u128 = 0;
    let mut f_total_sent: u128 = 0;
    let mut f_total_dropped: u128 = 0;
    for (from, to) in &segments {
        let key = format!("{}->{}", from, to);
        if let Some(stats) = channel_map.get(&key) {
            f_total_latency_us = f_total_latency_us.saturating_add(stats.total_latency_us as u128);
            f_total_sent = f_total_sent.saturating_add(stats.packets_sent as u128);
            f_total_dropped = f_total_dropped.saturating_add(stats.packets_dropped as u128);
        }
    }

    let latency_ms = if f_total_sent > 0 {
        (f_total_latency_us as f64) / (f_total_sent as f64) / 1000.0
    } else {
        0.0
    };
    let loss_pct = if f_total_sent + f_total_dropped > 0 {
        (f_total_dropped as f64) / ((f_total_sent + f_total_dropped) as f64) * 100.0
    } else {
        0.0
    };

    // Aggregate reverse (RSU -> selected) stats across reversed segments
    let mut r_total_latency_us: u128 = 0;
    let mut r_total_sent: u128 = 0;
    let mut r_total_dropped: u128 = 0;
    for (from, to) in &segments {
        let rev_key = format!("{}->{}", to, from);
        if let Some(stats) = channel_map.get(&rev_key) {
            r_total_latency_us = r_total_latency_us.saturating_add(stats.total_latency_us as u128);
            r_total_sent = r_total_sent.saturating_add(stats.packets_sent as u128);
            r_total_dropped = r_total_dropped.saturating_add(stats.packets_dropped as u128);
        }
    }

    let rev_latency_ms = if r_total_sent > 0 {
        (r_total_latency_us as f64) / (r_total_sent as f64) / 1000.0
    } else {
        0.0
    };
    let rev_loss_pct = if r_total_sent + r_total_dropped > 0 {
        (r_total_dropped as f64) / ((r_total_sent + r_total_dropped) as f64) * 100.0
    } else {
        0.0
    };

    let mut details = vec![
        Line::from(Span::raw(format!("Node: {}", node_name))),
        Line::from(Span::raw(format!("Path to RSU found: {}", reached_rsu))),
        Line::from(Span::raw(format!(
            "Est. Latency to RSU: {:.2} ms",
            latency_ms
        ))),
        Line::from(Span::raw(format!(
            "Est. Packet Loss to RSU: {:.2}%",
            loss_pct
        ))),
    ];

    // Append first-hop forward/reverse details if available
    if let Some((from, to)) = first_hop_from {
        let fwd_key = format!("{}->{}", from, to);
        let rev_key = format!("{}->{}", to, from);
        if let Some(fwd_stats) = channel_map.get(&fwd_key) {
            let f_latency_ms = if fwd_stats.packets_sent > 0 {
                (fwd_stats.total_latency_us as f64) / (fwd_stats.packets_sent as f64) / 1000.0
            } else {
                0.0
            };
            let f_loss = if fwd_stats.packets_sent + fwd_stats.packets_dropped > 0 {
                (fwd_stats.packets_dropped as f64)
                    / ((fwd_stats.packets_sent + fwd_stats.packets_dropped) as f64)
                    * 100.0
            } else {
                0.0
            };
            details.push(Line::from(Span::raw(format!(
                "Selected -> Upstream (first hop): {:.2} ms, loss {:.2}%",
                f_latency_ms, f_loss
            ))));
        }
        if let Some(rev_stats) = channel_map.get(&rev_key) {
            let r_latency_ms = if rev_stats.packets_sent > 0 {
                (rev_stats.total_latency_us as f64) / (rev_stats.packets_sent as f64) / 1000.0
            } else {
                0.0
            };
            let r_loss = if rev_stats.packets_sent + rev_stats.packets_dropped > 0 {
                (rev_stats.packets_dropped as f64)
                    / ((rev_stats.packets_sent + rev_stats.packets_dropped) as f64)
                    * 100.0
            } else {
                0.0
            };
            details.push(Line::from(Span::raw(format!(
                "Upstream -> Selected (first hop): {:.2} ms, loss {:.2}%",
                r_latency_ms, r_loss
            ))));
        }
    }

    // Append overall reverse path stats (RSU -> Selected)
    details.push(Line::from(Span::raw(format!(
        "Est. Reverse Latency (RSU -> Selected): {:.2} ms",
        rev_latency_ms
    ))));
    details.push(Line::from(Span::raw(format!(
        "Est. Reverse Packet Loss (RSU -> Selected): {:.2}%",
        rev_loss_pct
    ))));

    let para =
        Paragraph::new(details).block(Block::default().borders(Borders::ALL).title("Details"));
    f.render_widget(para, area);
}

/// Context for tree collection to reduce function arguments
struct TreeContext<'a> {
    state: &'a TopologyTabState<'a>,
    children: &'a BTreeMap<String, Vec<String>>,
    name_by_mac: &'a HashMap<String, String>,
    visited: &'a mut HashSet<String>,
    memo: &'a mut HashMap<String, usize>,
}

/// Helper to recursively collect nodes in tree order with proper prefixes
fn collect_node(
    name: &str,
    ctx: &mut TreeContext,
    out: &mut Vec<(String, String)>,
    prefix: &str,
    is_last: bool,
) {
    if ctx.visited.contains(name) {
        out.push((format!("{}[cycle to {}]", prefix, name), name.to_string()));
        return;
    }
    ctx.visited.insert(name.to_string());

    let label = if let Some((mac, _ntype, _snode)) = ctx.state.nodes.get(name) {
        format!("{} ({})", name, mac)
    } else {
        name.to_string()
    };

    if prefix.is_empty() {
        out.push((format!("└─ {}", label), name.to_string()));
    } else {
        let connector = if is_last { "└─ " } else { "├─ " };
        out.push((
            format!("{}{}{}", prefix, connector, label),
            name.to_string(),
        ));
    }

    if let Some(kids) = ctx.children.get(name) {
        let mut sorted_kids = kids.clone();
        sorted_kids.sort_by(|a, b| {
            let mut seen = HashSet::new();
            let da = compute_subtree_depth(a, ctx.children, ctx.memo, &mut seen);
            let mut seen = HashSet::new();
            let db = compute_subtree_depth(b, ctx.children, ctx.memo, &mut seen);
            match da.cmp(&db) {
                std::cmp::Ordering::Equal => {
                    let ha = compute_hops(a, ctx.state, ctx.name_by_mac).unwrap_or(u32::MAX);
                    let hb = compute_hops(b, ctx.state, ctx.name_by_mac).unwrap_or(u32::MAX);
                    match ha.cmp(&hb) {
                        std::cmp::Ordering::Equal => a.cmp(b),
                        other => other,
                    }
                }
                other => other,
            }
        });

        for (i, kid) in sorted_kids.iter().enumerate() {
            let last = i + 1 == sorted_kids.len();
            let new_prefix = if prefix.is_empty() {
                if is_last {
                    "   ".to_string()
                } else {
                    "│  ".to_string()
                }
            } else if is_last {
                format!("{}   ", prefix)
            } else {
                format!("{}│  ", prefix)
            };
            collect_node(kid, ctx, out, &new_prefix, last);
        }
    }

    ctx.visited.remove(name);
}

/// Compute hop distance from `name` to the nearest RSU by following cached_upstream_route
fn compute_hops(
    name: &str,
    state: &TopologyTabState,
    name_by_mac: &HashMap<String, String>,
) -> Option<u32> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut current = name.to_string();
    let mut total: u32 = 0;
    for _depth in 0..64 {
        if visited.contains(&current) {
            return None;
        }
        visited.insert(current.clone());

        // If current is RSU, we're done
        if let Some((_mac, ntype, _snode)) = state.nodes.get(&current) {
            if ntype == "Rsu" {
                return Some(total);
            }
        } else {
            return None;
        }

        // Otherwise, current must be an OBU; try to get its immediate upstream route
        if let Some((_mac, ntype, snode)) = state.nodes.get(&current) {
            if ntype != "Obu" {
                return None;
            }
            if let crate::simulator::SimNode::Obu(ref o) = snode {
                if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                    if let Some(route) = obu.cached_upstream_route() {
                        total = total.saturating_add(route.hops);
                        let immediate_mac = format!("{}", route.mac);
                        if let Some(next_name) = name_by_mac.get(&immediate_mac) {
                            current = next_name.clone();
                            continue;
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
    }
    None
}

/// Compute subtree depth (max distance to a leaf) for sorting so deeper branches can be placed last
fn compute_subtree_depth(
    name: &str,
    children: &BTreeMap<String, Vec<String>>,
    memo: &mut HashMap<String, usize>,
    seen: &mut HashSet<String>,
) -> usize {
    if let Some(&d) = memo.get(name) {
        return d;
    }
    if seen.contains(name) {
        // cycle -> treat as depth 0 to avoid infinite recursion
        memo.insert(name.to_string(), 0);
        return 0;
    }
    seen.insert(name.to_string());
    let mut max_child_depth: usize = 0;
    if let Some(kids) = children.get(name) {
        for kid in kids.iter() {
            let d = 1 + compute_subtree_depth(kid, children, memo, seen);
            if d > max_child_depth {
                max_child_depth = d;
            }
        }
    }
    seen.remove(name);
    memo.insert(name.to_string(), max_child_depth);
    max_child_depth
}
