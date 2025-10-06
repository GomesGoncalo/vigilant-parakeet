//! Terminal User Interface for real-time simulation monitoring
//!
//! Provides an interactive dashboard displaying:
//! - Packet statistics (sent, dropped, delayed)
//! - Performance metrics (drop rate, latency, throughput)
//! - Resource information (active nodes, channels)
//! - Live graphs showing trends over time
//! - Captured logs in a separate tab
//! - Interactive topology view

mod events;
mod logging;
mod render;
mod state;
mod tabs;
mod utils;

// Re-export public API
pub use logging::{LogBuffer, TuiLogLayer};

use crate::metrics::SimulatorMetrics;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use state::TuiState;
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::interval;

/// Run the TUI dashboard
///
/// This function takes over the terminal and displays a real-time dashboard
/// until the user presses 'q', 'Q', Esc, or Ctrl+C to quit.
pub async fn run_tui(
    metrics: Arc<SimulatorMetrics>,
    log_buffer: Arc<Mutex<VecDeque<String>>>,
    simulator: Arc<crate::simulator::Simulator>,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut state = TuiState::new(metrics, log_buffer);
    // Initial nodes snapshot
    state.refresh_nodes(&simulator);

    // Set up a panic hook to ensure terminal is restored even on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Run the TUI loop
    let res = run_tui_loop(&mut terminal, &mut state, simulator.clone()).await;

    // Restore terminal - always run this even if error occurred
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    // Restore original panic hook
    let _ = std::panic::take_hook();

    res
}

/// Main TUI event loop
async fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut TuiState,
    simulator: Arc<crate::simulator::Simulator>,
) -> Result<()> {
    let mut update_interval = interval(Duration::from_millis(250)); // Update 4 times per second
    update_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Handle events first with very short timeout for instant response
        if event::poll(Duration::from_millis(16))? {
            // ~60 FPS polling
            if let Event::Key(key) = event::read()? {
                // Handle both Press and Repeat events (some terminals only send one)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat {
                    // Delegate to events module
                    if events::handle_key_event(key, state)? {
                        return Ok(()); // Quit requested
                    }
                }
            }
        }

        // Update metrics and redraw periodically
        tokio::select! {
            _ = update_interval.tick() => {
                if !state.paused {
                    state.update();
                }
                // Refresh nodes every 1s to pick up topology changes
                if state.last_nodes_refresh.elapsed() > Duration::from_secs(1) {
                    state.refresh_nodes(&simulator);
                }
                terminal.draw(|f| render::render_ui(f, state))?;
            }
            // If no events, just redraw to keep UI responsive
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                terminal.draw(|f| render::render_ui(f, state))?;
            }
        }
    }
}
