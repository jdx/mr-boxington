//! Drawing the dashboard.

use super::app::{App, Row, Session, Tab};
use crate::events::{ActionOutcome, SessionState};
use crate::util::format_duration;
use bytesize::ByteSize;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row as TableRow, Table, Tabs};
use std::time::Duration;

/// The color an outcome is shown in.
///
/// A bypass is yellow rather than red: it is a deliberate refusal to cache, not
/// a failure, and coloring it like one would teach the wrong lesson about a
/// perfectly healthy build.
fn outcome_style(outcome: &ActionOutcome) -> Style {
    match outcome {
        ActionOutcome::Hit => Style::new().fg(Color::Green),
        ActionOutcome::Miss => Style::new().fg(Color::Red),
        ActionOutcome::Unconsulted => Style::new().fg(Color::DarkGray),
        ActionOutcome::Verification { matched: true } => Style::new().fg(Color::Cyan),
        ActionOutcome::Verification { matched: false } => Style::new().fg(Color::Magenta).bold(),
        ActionOutcome::Bypass { .. } => Style::new().fg(Color::Yellow),
    }
}

fn state_style(state: SessionState) -> Style {
    match state {
        SessionState::Live => Style::new().fg(Color::Green).bold(),
        SessionState::Finished => Style::new().fg(Color::DarkGray),
        SessionState::Abandoned => Style::new().fg(Color::Red),
    }
}

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    header(frame, areas[0], app);
    tabs(frame, areas[1], app);
    match app.tab {
        Tab::Live => live(frame, areas[2], app),
        Tab::Sessions => sessions(frame, areas[2], app),
        Tab::Store => store(frame, areas[2], app),
    }
    footer(frame, areas[3], app);
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        app.store_dir().display().to_string(),
        Style::new().fg(Color::Cyan),
    )];
    if let Some(stats) = &app.store_stats {
        spans.push(Span::raw("  ·  "));
        spans.push(Span::raw(format!(
            "{} in {} objects",
            ByteSize::b(stats.total_bytes()).display().iec(),
            stats.objects
        )));
    }
    let saved = Duration::from_nanos(app.savings.avoided_compiler_ns);
    if !saved.is_zero() {
        spans.push(Span::raw("  ·  "));
        spans.push(Span::styled(
            format!("{} of compiling saved so far", format_duration(saved)),
            Style::new().fg(Color::Green),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::ALL).title(" mbx ")),
        area,
    );
}

fn tabs(frame: &mut Frame, area: Rect, app: &App) {
    let selected = Tab::ALL.iter().position(|tab| *tab == app.tab).unwrap_or(0);
    frame.render_widget(
        Tabs::new(Tab::ALL.map(Tab::title).to_vec())
            .select(selected)
            .highlight_style(Style::new().bold().fg(Color::Cyan))
            .divider(" "),
        area,
    );
}

/// The live view: which builds are running, and what the selected one is doing.
fn live(frame: &mut Frame, area: Rect, app: &App) {
    if app.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No builds recorded yet.\n\nRun `mbx build` in another terminal and it will appear here.",
            )
            .block(Block::default().borders(Borders::ALL).title(" builds ")),
            area,
        );
        return;
    }
    let areas = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(area);
    session_list(frame, areas[0], app);
    if let Some(session) = app.selected_session() {
        action_rows(frame, areas[1], session);
    }
}

fn session_list(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<TableRow> = app
        .sessions()
        .enumerate()
        .map(|(index, session)| {
            let marker = if index == app.selected { "▸" } else { " " };
            let hit_rate = session
                .hit_rate()
                .map(|rate| format!("{rate:.0}%"))
                .unwrap_or_else(|| "-".into());
            TableRow::new(vec![
                Cell::from(marker),
                Cell::from(session.title()),
                Cell::from(session.workspace_name().unwrap_or("").to_string()),
                Cell::from(Span::styled(
                    session.state.label(),
                    state_style(session.state),
                )),
                Cell::from(Span::styled(
                    session.count("hit").to_string(),
                    Style::new().fg(Color::Green),
                )),
                Cell::from(Span::styled(
                    session.count("miss").to_string(),
                    Style::new().fg(Color::Red),
                )),
                Cell::from(Span::styled(
                    session.count("unconsulted").to_string(),
                    Style::new().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    session
                        .bypasses()
                        .iter()
                        .map(|(_, count)| count)
                        .sum::<u64>()
                        .to_string(),
                    Style::new().fg(Color::Yellow),
                )),
                Cell::from(hit_rate),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(20),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
    )
    .header(
        TableRow::new(vec![
            "",
            "command",
            "workspace",
            "state",
            "hit",
            "miss",
            "unconsulted",
            "bypass",
            "rate",
        ])
        .style(Style::new().bold()),
    )
    .block(Block::default().borders(Borders::ALL).title(" builds "));
    frame.render_widget(table, area);
}

fn action_rows(frame: &mut Frame, area: Rect, session: &Session) {
    let title = match session.truncated {
        true => format!(" {} (history capped) ", session.title()),
        false => format!(" {} ", session.title()),
    };
    // The visible window is the tail of the build: what just happened is what a
    // watcher is looking for, so rows are shown newest-last and scrolled to the
    // end.
    let height = area.height.saturating_sub(2) as usize;
    let start = session.rows.len().saturating_sub(height);
    let rows: Vec<TableRow> = session.rows[start..]
        .iter()
        .map(|row| {
            let Row {
                outcome,
                crate_name,
                duration_ns,
            } = row;
            TableRow::new(vec![
                Cell::from(Span::styled(
                    outcome.label().to_string(),
                    outcome_style(outcome),
                )),
                Cell::from(crate_name.clone().unwrap_or_else(|| "-".into())),
                Cell::from(format_duration(Duration::from_nanos(*duration_ns))),
            ])
        })
        .collect();
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(24),
                Constraint::Min(20),
                Constraint::Length(10),
            ],
        )
        .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

/// Finished builds, read from the totals each stream ends with.
fn sessions(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<TableRow> = app
        .sessions()
        .skip(app.scroll)
        .map(|session| {
            let totals = session.totals.as_ref();
            let field = |key: &str| {
                totals
                    .map(|totals| crate::events::stat(totals, key))
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into())
            };
            let saved = totals
                .map(|totals| crate::events::stat(totals, "estimated_compiler_duration_avoided_ns"))
                .map(|ns| format_duration(Duration::from_nanos(ns)))
                .unwrap_or_else(|| "-".into());
            TableRow::new(vec![
                Cell::from(session.title()),
                Cell::from(Span::styled(
                    session.state.label(),
                    state_style(session.state),
                )),
                Cell::from(field("hits")),
                Cell::from(field("misses")),
                Cell::from(field("unconsulted")),
                Cell::from(saved),
            ])
        })
        .collect();
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(24),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
        )
        .header(
            TableRow::new(vec![
                "command",
                "state",
                "hits",
                "misses",
                "unconsulted",
                "saved",
            ])
            .style(Style::new().bold()),
        )
        .block(Block::default().borders(Borders::ALL).title(" sessions ")),
        area,
    );
}

fn store(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    match &app.store_stats {
        Some(stats) => {
            lines.push(Line::from(format!(
                "objects:        {} ({})",
                stats.objects,
                ByteSize::b(stats.object_bytes).display().iec()
            )));
            lines.push(Line::from(format!(
                "action results: {} ({})",
                stats.action_results,
                ByteSize::b(stats.action_result_bytes).display().iec()
            )));
            lines.push(Line::from(format!(
                "checkouts:      {} live, {} stale",
                stats.live_checkouts, stats.stale_checkouts
            )));
            lines.push(Line::from(format!(
                "total:          {}",
                ByteSize::b(stats.total_bytes()).display().iec()
            )));
        }
        None => lines.push(Line::from("the store could not be read")),
    }
    let tally = &app.savings;
    if tally.builds > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "since mbx started counting",
            Style::new().bold(),
        )));
        lines.push(Line::from(format!("builds:         {}", tally.builds)));
        lines.push(Line::from(format!(
            "compilations:   {} restored",
            tally.cached_compilations
        )));
        lines.push(Line::from(format!(
            "compiling:      {} saved",
            format_duration(Duration::from_nanos(tally.avoided_compiler_ns))
        )));
        lines.push(Line::from(format!(
            "target/:        {} collected",
            ByteSize::b(tally.freed_target_bytes).display().iec()
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" store ")),
        area,
    );
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let keys = if app.paused {
        "paused — p resume · tab switch · j/k move · q quit"
    } else {
        "p pause · tab switch · 1-3 jump · j/k move · q quit"
    };
    frame.render_widget(
        Paragraph::new(Span::styled(keys, Style::new().fg(Color::DarkGray))),
        area,
    );
}
