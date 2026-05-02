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
    let connected = state.connected_clients.load(Ordering::Relaxed);
    let max = state.max_clients;

    // Outer vertical split: main row above, hints bar below.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(frame.area());

    // Main row split into two columns: pipeline status left, server telemetry right.
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(rows[0]);

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

    let available = max.saturating_sub(connected);
    let clients_text = vec![
        Line::from(vec![
            Span::raw("  Connected   "),
            Span::styled(
                format!("{connected}"),
                Style::default()
                    .fg(if connected > 0 {
                        Color::Green
                    } else {
                        Color::DarkGray
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  Available   "),
            Span::styled(format!("{available}"), Style::default().fg(Color::DarkGray)),
        ]),
    ];

    let clients = Paragraph::new(clients_text)
        .block(Block::default().borders(Borders::ALL).title(" Clients "));

    let hints = Paragraph::new(Line::from(
        "  A: analyse   B: broadcast   R: record   Q / Ctrl+C: quit",
    ))
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(status, columns[0]);
    frame.render_widget(clients, columns[1]);
    frame.render_widget(hints, rows[1]);
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
            format!("Record overruns: {count}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("Record overruns: 0", Style::default().fg(Color::DarkGray))
    }
}
