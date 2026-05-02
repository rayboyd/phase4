//! The running application control surface, displayed once [`phase4::app::App`]
//! has been constructed and the pipeline is live.

use phase4::app::AppState;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Renders the control surface into the given frame.
pub fn render(frame: &mut Frame, state: &Arc<AppState>) {
    let analysing = state.is_analysing.load(Ordering::Relaxed);
    let broadcasting = state.is_broadcasting_websocket.load(Ordering::Relaxed);
    let recording = state.is_recording.load(Ordering::Relaxed);
    let overflows = state.record_ring_overflow_events.load(Ordering::Relaxed);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(frame.area());

    let status_line = Line::from(vec![
        status_span("Analyse", analysing),
        Span::raw("   "),
        status_span("Broadcast", broadcasting),
        Span::raw("   "),
        status_span("Record", recording),
        Span::raw("   "),
        overflow_span(overflows),
    ]);

    let status =
        Paragraph::new(status_line).block(Block::default().borders(Borders::ALL).title(" Phase4 "));

    let hints = Paragraph::new(Line::from(
        "  A: analyse   B: broadcast   R: record   Q / Ctrl+C: quit",
    ))
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(status, chunks[0]);
    frame.render_widget(hints, chunks[1]);
}

fn status_span(label: &'static str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            format!("{label} [ON] "),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{label} [OFF]"),
            Style::default().fg(Color::DarkGray),
        )
    }
}

fn overflow_span(count: usize) -> Span<'static> {
    if count > 0 {
        Span::styled(
            format!("Ring overflows: {count}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("Ring overflows: 0", Style::default().fg(Color::DarkGray))
    }
}
