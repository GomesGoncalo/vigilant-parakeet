// Upstreams tab rendering - shows OBU routing to RSUs
use crate::tui::state::TuiState;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

/// Render the upstreams tab showing OBU upstream routing information
pub fn render_upstreams_tab(f: &mut Frame, area: Rect, state: &TuiState) {
    // If paused and we have a snapshot, use that; otherwise compute live entries from state.nodes
    let rows: Vec<Row> = if state.paused {
        if let Some(ref ups) = state.paused_upstreams {
            let mut entries: Vec<(String, Vec<Cell>)> = ups
                .iter()
                .map(|(name, obu_mac, up_display, up_mac, hops, next_hop)| {
                    let obu_label = format!("{} ({})", name, obu_mac);
                    let up_label = if up_display.starts_with("(") || up_display.contains(':') {
                        // if up_display is a mac or placeholder like (no upstream), prefer showing name + mac when possible
                        if up_display.starts_with('(') {
                            format!(
                                "{} ({})",
                                up_display.trim_matches(|c| c == '(' || c == ')'),
                                up_mac
                            )
                        } else {
                            format!("{} ({})", up_display, up_mac)
                        }
                    } else {
                        format!("{} ({})", up_display, up_mac)
                    };
                    let cells = vec![
                        Cell::from(obu_label),
                        Cell::from(up_label),
                        Cell::from(hops.clone()),
                        Cell::from(next_hop.clone()),
                    ];
                    (name.clone(), cells)
                })
                .collect();

            // Ensure alphabetical order while paused
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
                .into_iter()
                .map(|(_n, cells)| Row::new(cells).height(1))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        // Build live entries from state.nodes where node_type == "Obu", then sort by OBU name
        let mut entries: Vec<(String, Vec<Cell>)> = Vec::new();

        for (name, (mac, node_type, simnode)) in state.nodes.iter() {
            if node_type != "Obu" {
                continue;
            }

            // Build labels with MAC addresses
            let obu_label = format!("{} ({})", name, mac);

            // Try to downcast to Obu to get cached upstream route
            let mut upstream_display = "(no upstream)".to_string();
            let mut hops = "-".to_string();
            let mut next_hop = "-".to_string();

            // Use SimNode's as_any to downcast to obu_lib::Obu
            if let crate::simulator::SimNode::Obu(ref o) = simnode {
                // oba is Arc<dyn Node>; try downcast via as_any
                if let Some(obu) = o.as_any().downcast_ref::<obu_lib::Obu>() {
                    if let Some(route) = obu.cached_upstream_route() {
                        // Immediate next hop (first hop on path)
                        let immediate_next = format!("{}", route.mac);
                        next_hop = immediate_next.clone();

                        // Try to resolve final RSU by walking the upstream chain.
                        let mut total_hops = route.hops;
                        let mut current_mac = format!("{}", route.mac);
                        let mut depth = 0;
                        let final_name = loop {
                            if depth > 16 {
                                break None;
                            }
                            depth += 1;
                            if let Some((nname, (_m, ntype, snode))) =
                                state.nodes.iter().find(|(_, (m, _, _))| **m == current_mac)
                            {
                                if ntype == "Rsu" {
                                    break Some(nname.clone());
                                }
                                if ntype == "Obu" {
                                    if let crate::simulator::SimNode::Obu(ref other_o) = snode {
                                        if let Some(other_obu) =
                                            other_o.as_any().downcast_ref::<obu_lib::Obu>()
                                        {
                                            if let Some(next_route) =
                                                other_obu.cached_upstream_route()
                                            {
                                                total_hops =
                                                    total_hops.saturating_add(next_route.hops);
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

            // If upstream_display is a name, try to lookup its mac for display
            let upstream_label =
                if upstream_display.starts_with('(') || upstream_display.contains(':') {
                    // Either (no upstream) or a MAC
                    upstream_display.to_string()
                } else {
                    // Lookup by name
                    if let Some((_, (umac, _, _))) =
                        state.nodes.iter().find(|(n, _)| *n == &upstream_display)
                    {
                        format!("{} ({})", upstream_display, umac)
                    } else {
                        upstream_display.to_string()
                    }
                };

            let cells = vec![
                Cell::from(obu_label),
                Cell::from(upstream_label),
                Cell::from(hops),
                Cell::from(next_hop),
            ];
            entries.push((name.clone(), cells));
        }

        // Sort alphabetically by OBU name
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        // Convert to table rows
        entries
            .into_iter()
            .map(|(_name, cells)| Row::new(cells).height(1))
            .collect()
    };

    let header = Row::new(vec![
        Cell::from("OBU"),
        Cell::from("Upstream RSU"),
        Cell::from("Hops"),
        Cell::from("Next Hop MAC"),
    ])
    .bottom_margin(1);

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("OBU Upstreams"),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(table, area);
}
