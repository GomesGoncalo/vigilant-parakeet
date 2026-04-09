// Upstreams tab rendering - shows OBU routing to RSUs
use crate::tui::state::{TuiState, UpstreamSnapshot};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};
use std::collections::HashMap;

use super::TabRenderer;

/// State data for the Upstreams tab
pub struct UpstreamsTabState<'a> {
    pub nodes: &'a HashMap<String, crate::tui::state::NodeSnapshot>,
    pub paused: bool,
    pub paused_upstreams: &'a Option<Vec<UpstreamSnapshot>>,
}

/// Upstreams tab renderer
pub struct UpstreamsTab;

impl TabRenderer for UpstreamsTab {
    type State<'a> = UpstreamsTabState<'a>;

    fn display_name() -> &'static str {
        "🔼 Upstreams"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        UpstreamsTabState {
            nodes: &tui_state.nodes,
            paused: tui_state.paused,
            paused_upstreams: &tui_state.paused_upstreams,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        UpstreamsTabState {
            nodes: &tui_state.nodes,
            paused: tui_state.paused,
            paused_upstreams: &tui_state.paused_upstreams,
        }
    }

    fn render(f: &mut Frame, area: Rect, state: Self::State<'_>) {
        // If paused and we have a snapshot, use that; otherwise compute live entries from state.nodes
        let rows: Vec<Row> = if state.paused {
            if let Some(ref ups) = state.paused_upstreams {
                let mut entries: Vec<(&str, Row)> = ups
                    .iter()
                    .map(|snap| {
                        let obu_label =
                            format!("{} ({})", snap.obu_name, snap.obu_mac);
                        let up_label =
                            if snap.upstream_display.starts_with('(')
                                || snap.upstream_display.contains(':')
                            {
                                if snap.upstream_display.starts_with('(') {
                                    format!(
                                        "{} ({})",
                                        snap.upstream_display
                                            .trim_matches(|c| c == '(' || c == ')'),
                                        snap.upstream_mac
                                    )
                                } else {
                                    format!(
                                        "{} ({})",
                                        snap.upstream_display, snap.upstream_mac
                                    )
                                }
                            } else {
                                format!("{} ({})", snap.upstream_display, snap.upstream_mac)
                            };
                        let row = Row::new(vec![
                            Cell::from(obu_label),
                            Cell::from(up_label),
                            Cell::from(snap.hops.clone()),
                            Cell::from(snap.next_hop_mac.clone()),
                        ])
                        .height(1);
                        (snap.obu_name.as_str(), row)
                    })
                    .collect();

                // Ensure alphabetical order while paused
                entries.sort_by(|a, b| a.0.cmp(b.0));
                entries.into_iter().map(|(_, row)| row).collect()
            } else {
                Vec::new()
            }
        } else {
            // Build live entries from state.nodes where node_type == "Obu", then sort by OBU name
            let mut entries: Vec<(String, Vec<Cell>)> = Vec::new();

            for (name, snap) in state.nodes.iter() {
                if snap.node_type != "Obu" {
                    continue;
                }

                // Build labels with MAC addresses
                let obu_label = format!("{} ({})", name, snap.mac);

                // Try to downcast to Obu to get cached upstream route
                let mut upstream_display = "(no upstream)".to_string();
                let mut hops = "-".to_string();
                let mut next_hop = "-".to_string();

                // Use SimNode's as_any to downcast to obu_lib::Obu
                if let crate::simulator::SimNode::Obu(ref o) = snap.simnode {
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
                                if let Some((nname, nsnap)) = state
                                    .nodes
                                    .iter()
                                    .find(|(_, s)| s.mac == current_mac)
                                {
                                    if nsnap.node_type == "Rsu" {
                                        break Some(nname.clone());
                                    }
                                    if nsnap.node_type == "Obu" {
                                        if let crate::simulator::SimNode::Obu(ref other_o) =
                                            nsnap.simnode
                                        {
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
                            upstream_display =
                                final_name.unwrap_or_else(|| format!("{}", route.mac));
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
                        if let Some((_, (umac, _ntype, _v, _c, _snode))) =
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

    fn help_text(_state: Self::State<'_>) -> Vec<Span<'static>> {
        vec![
            super::key_span("Q/Esc/Ctrl+C"),
            super::text_span(" quit  │  "),
            super::key_span("P"),
            super::text_span(" pause  │  "),
            super::key_span("Tab/1/2/3/4/5"),
            super::text_span(" switch tabs"),
        ]
    }
}
