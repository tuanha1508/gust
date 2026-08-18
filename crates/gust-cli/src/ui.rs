//! Live terminal dashboard for a Gust run.
//!
//! Throughput, raw-vs-corrected percentiles, windowed latency chart, knee
//! marker, and in-flight depth (backpressure / Little’s Law intuition).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use gust_core::{Knee, StepSummary, Summary, WindowMetric};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table};
use ratatui::{DefaultTerminal, Frame as RFrame};

use crate::{RunInfo, UiState};

const P50: Color = Color::Green;
const P90: Color = Color::Yellow;
const P99: Color = Color::LightRed;
const KNEE: Color = Color::Cyan;
const INFLIGHT: Color = Color::Magenta;

pub fn run_dashboard(
    info: &RunInfo,
    sent: Arc<AtomicU64>,
    state: Arc<Mutex<UiState>>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, info, &sent, &state, &stop);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    info: &RunInfo,
    sent: &Arc<AtomicU64>,
    state: &Arc<Mutex<UiState>>,
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        let (snapshot, finished) = {
            let s = state.lock().unwrap();
            (
                Snapshot {
                    summary: s.summary,
                    steps: s.steps.clone(),
                    series: s.series.clone(),
                    throughput: s.throughput,
                    target_rps: s.target_rps,
                    in_flight: s.in_flight,
                    knee: s.knee.clone(),
                    stopping: s.stopping,
                },
                s.finished,
            )
        };
        let sent_n = sent.load(Ordering::Relaxed);

        terminal.draw(|f| draw(f, info, sent_n, &snapshot, finished))?;

        if should_quit()? {
            if finished {
                // Second q (or q after a natural end): leave the dashboard.
                return Ok(());
            }
            // First q while running: stop scheduling and drain in-flight.
            // Stay on the dashboard until the recorder marks finished.
            stop.store(true, Ordering::Relaxed);
            if let Ok(mut s) = state.lock() {
                s.stopping = true;
            }
        }
    }
}

fn should_quit() -> Result<bool> {
    if event::poll(Duration::from_millis(33))?
        && let Event::Key(key) = event::read()?
        && key.kind == KeyEventKind::Press
    {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Snapshot {
    summary: Option<Summary>,
    steps: Vec<StepSummary>,
    series: Vec<WindowMetric>,
    throughput: f64,
    target_rps: f64,
    in_flight: u64,
    knee: Option<Knee>,
    stopping: bool,
}

fn draw(f: &mut RFrame, info: &RunInfo, sent: u64, snap: &Snapshot, finished: bool) {
    let show_steps = snap.steps.len() > 1;
    let table_h = if show_steps { 7u16 } else { 9 };
    let steps_h = if show_steps {
        (snap.steps.len() as u16 + 3).min(8)
    } else {
        0
    };

    let [header, knee_row, stats, tables, charts, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(table_h + steps_h),
        Constraint::Min(10),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let (pct_area, steps_area) = if show_steps {
        let [a, b] = Layout::vertical([Constraint::Length(table_h), Constraint::Length(steps_h)])
            .areas(tables);
        (a, Some(b))
    } else {
        (tables, None)
    };

    let [latency, inflight] =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(charts);

    draw_header(f, header, info, finished, snap.stopping);
    draw_knee(f, knee_row, snap);
    draw_stats(f, stats, sent, snap);
    draw_table(f, pct_area, snap);
    if let Some(area) = steps_area {
        draw_steps(f, area, snap);
    }
    draw_latency_chart(f, latency, info, snap);
    draw_inflight_chart(f, inflight, info, snap);

    let hint = if finished {
        "finished — press q to exit".to_string()
    } else if snap.stopping {
        format!(
            "stopping — draining {} in-flight… (summary follows)",
            snap.in_flight
        )
    } else {
        "running — press q to stop".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {hint}"),
            Style::default().fg(Color::DarkGray),
        ))),
        footer,
    );
}

fn draw_header(
    f: &mut RFrame,
    area: ratatui::layout::Rect,
    info: &RunInfo,
    finished: bool,
    stopping: bool,
) {
    let (status, status_color) = if finished {
        ("DONE", Color::Green)
    } else if stopping {
        ("STOPPING", Color::Yellow)
    } else {
        ("LIVE", Color::Cyan)
    };
    let title = Line::from(vec![
        Span::styled(
            " gust ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {} · {} · {}s  ",
            info.url, info.profile_label, info.duration
        )),
        Span::styled(
            format!("[{status}]"),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_knee(f: &mut RFrame, area: ratatui::layout::Rect, snap: &Snapshot) {
    // Nothing was served, so there is no capacity to report — say what broke.
    let all_failed = snap
        .summary
        .as_ref()
        .is_some_and(|s| s.total > 0 && s.success == 0);
    if all_failed {
        let detail = crate::first_failure().unwrap_or("every request failed");
        let line = Line::from(vec![
            Span::styled("  NO CAPACITY ", bold(Color::LightRed)),
            Span::raw(format!("every request failed · {detail}")),
        ]);
        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let line = match &snap.knee {
        Some(k) => Line::from(vec![
            Span::styled("  KNEE ", bold(KNEE)),
            Span::styled(format!("≈ {:.0} req/s", k.target_rps), bold(Color::White)),
            Span::raw(format!(
                " at {:.1}s · stay under {:.0} req/s · {}",
                k.t, k.recommended_rps, k.reason
            )),
        ]),
        None => Line::from(Span::styled(
            "  knee: watching for break (p99 spike / errors / throughput stall)…",
            Style::default().fg(Color::DarkGray),
        )),
    };
    f.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_stats(f: &mut RFrame, area: ratatui::layout::Rect, sent: u64, snap: &Snapshot) {
    let (completed, success, failure) = match &snap.summary {
        Some(s) => (s.total, s.success, s.failure),
        None => (0, 0, 0),
    };
    let pct = |n: u64| {
        if completed == 0 {
            0.0
        } else {
            n as f64 / completed as f64 * 100.0
        }
    };

    let line = Line::from(vec![
        Span::raw("  arr "),
        Span::styled(sent.to_string(), bold(Color::White)),
        Span::raw("   done "),
        Span::styled(completed.to_string(), bold(Color::White)),
        Span::raw("   ok "),
        Span::styled(format!("{:.1}%", pct(success)), bold(Color::Green)),
        Span::raw("   fail "),
        Span::styled(
            format!("{:.1}%", pct(failure)),
            bold(if failure > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw("   target "),
        Span::styled(format!("{:.0}/s", snap.target_rps), bold(Color::DarkGray)),
        Span::raw("   thr "),
        Span::styled(format!("{:.0}/s", snap.throughput), bold(Color::Cyan)),
        Span::raw("   in-flight "),
        Span::styled(
            snap.in_flight.to_string(),
            bold(if snap.in_flight > 0 {
                INFLIGHT
            } else {
                Color::DarkGray
            }),
        ),
    ]);
    f.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_table(f: &mut RFrame, area: ratatui::layout::Rect, snap: &Snapshot) {
    let rows = match &snap.summary {
        Some(s) => vec![
            pct_row_raw_only("min", s.raw.min_ms),
            pct_row("p50", s.raw.p50_ms, s.corrected.p50_ms),
            pct_row("p90", s.raw.p90_ms, s.corrected.p90_ms),
            pct_row("p99", s.raw.p99_ms, s.corrected.p99_ms),
            pct_row("p99.9", s.raw.p999_ms, s.corrected.p999_ms),
            pct_row("max", s.raw.max_ms, s.corrected.max_ms),
        ],
        None => vec![],
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(14),
        ],
    )
    .header(Row::new(vec!["latency", "raw (ms)", "corrected (ms)"]).style(bold(Color::White)))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" cumulative percentiles "),
    );
    f.render_widget(table, area);
}

fn pct_row(label: &str, raw: f64, corrected: f64) -> Row<'static> {
    let gap_color = if corrected > raw * 1.5 {
        Color::LightRed
    } else {
        Color::Gray
    };
    Row::new(vec![
        Span::styled(label.to_string(), Style::default().fg(Color::White)),
        Span::styled(format!("{raw:.2}"), Style::default().fg(Color::Gray)),
        Span::styled(format!("{corrected:.2}"), Style::default().fg(gap_color)),
    ])
}

fn pct_row_raw_only(label: &str, raw: f64) -> Row<'static> {
    Row::new(vec![
        Span::styled(label.to_string(), Style::default().fg(Color::White)),
        Span::styled(format!("{raw:.2}"), Style::default().fg(Color::Gray)),
        Span::styled("—".to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

fn draw_steps(f: &mut RFrame, area: ratatui::layout::Rect, snap: &Snapshot) {
    let rows: Vec<Row> = snap
        .steps
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let ss = &st.summary;
            let style = if i == 0 {
                Style::default().fg(Color::LightRed)
            } else {
                Style::default().fg(Color::Gray)
            };
            Row::new(vec![
                Span::styled(st.name.clone(), Style::default().fg(Color::White)),
                Span::styled(ss.total.to_string(), Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{:.1}", ss.raw.p50_ms),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(format!("{:.1}", ss.raw.p99_ms), style),
                Span::styled(format!("{:.1}", ss.corrected.p99_ms), style),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(vec!["step", "n", "p50", "p99", "p99 corr"]).style(bold(Color::White)))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" by step (slowest first) "),
    );
    f.render_widget(table, area);
}

fn draw_latency_chart(
    f: &mut RFrame,
    area: ratatui::layout::Rect,
    info: &RunInfo,
    snap: &Snapshot,
) {
    let p50: Vec<(f64, f64)> = snap.series.iter().map(|p| (p.t, p.p50_ms)).collect();
    let p90: Vec<(f64, f64)> = snap.series.iter().map(|p| (p.t, p.p90_ms)).collect();
    let p99: Vec<(f64, f64)> = snap.series.iter().map(|p| (p.t, p.p99_ms)).collect();

    let y_max = snap.series.iter().map(|p| p.p99_ms).fold(1.0_f64, f64::max) * 1.15;
    let x_max = info.duration as f64;

    let knee_line: Vec<(f64, f64)> = match &snap.knee {
        Some(k) => vec![(k.t, 0.0), (k.t, y_max)],
        None => vec![],
    };

    let mut datasets = vec![
        line_dataset("p50", P50, &p50),
        line_dataset("p90", P90, &p90),
        line_dataset("p99", P99, &p99),
    ];
    if !knee_line.is_empty() {
        datasets.push(line_dataset("knee", KNEE, &knee_line));
    }

    let title = match &snap.knee {
        Some(k) => format!(" latency · knee ≈ {:.0} req/s ", k.target_rps),
        None => " latency over time (windowed) ".into(),
    };

    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", x_max / 2.0)),
                    Span::raw(format!("{x_max:.0}")),
                ]),
        )
        .y_axis(
            Axis::default()
                .title("ms")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", y_max / 2.0)),
                    Span::raw(format!("{y_max:.0}")),
                ]),
        );
    f.render_widget(chart, area);
}

fn draw_inflight_chart(
    f: &mut RFrame,
    area: ratatui::layout::Rect,
    info: &RunInfo,
    snap: &Snapshot,
) {
    let series: Vec<(f64, f64)> = snap.series.iter().map(|p| (p.t, p.in_flight)).collect();
    let y_max = snap
        .series
        .iter()
        .map(|p| p.in_flight)
        .fold(1.0_f64, f64::max)
        .max(snap.in_flight as f64)
        * 1.15;
    let x_max = info.duration as f64;

    let chart = Chart::new(vec![line_dataset("in-flight", INFLIGHT, &series)])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" in-flight requests (backpressure) "),
        )
        .x_axis(
            Axis::default()
                .title("t (s)")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, x_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", x_max / 2.0)),
                    Span::raw(format!("{x_max:.0}")),
                ]),
        )
        .y_axis(
            Axis::default()
                .title("n")
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(format!("{:.0}", y_max / 2.0)),
                    Span::raw(format!("{y_max:.0}")),
                ]),
        );
    f.render_widget(chart, area);
}

fn line_dataset<'a>(name: &'a str, color: Color, data: &'a [(f64, f64)]) -> Dataset<'a> {
    Dataset::default()
        .name(name)
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(data)
}

fn bold(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}
