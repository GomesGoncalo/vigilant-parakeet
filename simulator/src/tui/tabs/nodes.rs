// Nodes tab rendering - list of nodes and details
use super::TabRenderer;
use crate::tui::state::TuiState;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};
// Ipv4Addr is referenced via the NodeSnapshot alias; no direct import needed here

pub struct NodesTabState<'a> {
    pub nodes: &'a crate::tui::state::NodesMap,
    pub selected_index: &'a mut usize,
    pub item_count: &'a mut usize,
}

pub struct NodesTab;

impl TabRenderer for NodesTab {
    type State<'a> = NodesTabState<'a>;

    fn display_name() -> &'static str {
        "🧭 Nodes"
    }

    fn extract_state<'a>(tui_state: &'a mut TuiState) -> Self::State<'a> {
        NodesTabState {
            nodes: &tui_state.nodes,
            selected_index: &mut tui_state.selected_topology_index,
            item_count: &mut tui_state.topology_item_count,
        }
    }

    fn extract_help_state<'a>(tui_state: &'a TuiState) -> Self::State<'a> {
        static mut DUMMY_INDEX: usize = 0;
        static mut DUMMY_COUNT: usize = 0;
        #[allow(static_mut_refs)]
        NodesTabState {
            nodes: &tui_state.nodes,
            selected_index: unsafe { &mut DUMMY_INDEX },
            item_count: unsafe { &mut DUMMY_COUNT },
        }
    }

    fn render(f: &mut Frame, area: Rect, mut state: Self::State<'_>) {
        render_nodes_tab(f, area, &mut state);
    }

    fn help_text(_state: Self::State<'_>) -> Vec<Span<'static>> {
        vec![
            super::key_span("Q/Esc/Ctrl+C"),
            super::text_span(" quit  │  "),
            super::key_span("↑/↓/PgUp/PgDn"),
            super::text_span(" navigate  │  "),
            super::key_span("Enter"),
            super::text_span(" details  │  "),
        ]
    }
}

fn render_nodes_tab(f: &mut Frame, area: Rect, state: &mut NodesTabState) {
    let mut rows: Vec<Row> = Vec::new();
    let mut names: Vec<String> = state.nodes.keys().cloned().collect();
    names.sort();

    for name in names.iter() {
        if let Some((mac, ntype, v_ip, c_ip, _snode)) = state.nodes.get(name) {
            let ip_display = match (v_ip, c_ip) {
                (Some(v), Some(c)) => format!("{} / {}", v, c),
                (Some(v), None) => format!("{}", v),
                (None, Some(c)) => format!("{}", c),
                (None, None) => "-".to_string(),
            };
            let health = "ok";
            let health_style = Style::default().fg(Color::Green);
            let health_cell = Cell::from(Span::styled(health.to_string(), health_style));
            let row = Row::new(vec![
                Cell::from(name.to_string()),
                Cell::from(ntype.clone()),
                Cell::from(mac.clone()),
                Cell::from(ip_display),
                Cell::from("-"),
                health_cell,
            ])
            .height(1);
            rows.push(row);
        }
    }

    *state.item_count = rows.len();

    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Type"),
        Cell::from("MAC"),
        Cell::from("IP(s)"),
        Cell::from("Last"),
        Cell::from("Health"),
    ])
    .bottom_margin(1);

    let widths = [
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
        Constraint::Percentage(25),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Nodes"))
        .style(Style::default().fg(Color::White));

    // selection handling
    let mut stateful = ratatui::widgets::TableState::default();
    if *state.item_count > 0 && *state.selected_index < *state.item_count {
        stateful.select(Some(*state.selected_index));
    } else {
        stateful.select(None);
    }

    f.render_stateful_widget(table, area, &mut stateful);

    // details pane is omitted here since NodesTab is compact; could render details on Enter in future
}
