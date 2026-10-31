use std::io;
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::logger::Logger;
use crate::manager::{EndpointView, Snapshot, Ui};
use crate::platform;
use crate::startup;

const REFRESH_INTERVAL: Duration = Duration::from_millis(200);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const MUTED: Color = Color::DarkGray;

#[derive(Default)]
struct HitRegions {
    list: Rect,
    list_offset: usize,
    rows: usize,
}

struct View<'a> {
    snapshot: &'a Snapshot,
    selected: usize,
    os: &'a str,
    autostart: bool,
}

pub enum Outcome {
    Quit,
    Tray,
}

pub fn run(ui: &Ui, logger: &Logger) -> Result<Outcome, String> {
    logger.start_tui();
    logger.set_console_title(&format!(
        "real-rs {} - reduce audio latency",
        env!("CARGO_PKG_VERSION")
    ));
    let outcome = run_terminal(ui);
    logger.stop_tui();
    outcome
}

fn run_terminal(ui: &Ui) -> Result<Outcome, String> {
    enable_raw_mode().map_err(|error| format!("could not enable raw mode: {error}"))?;
    let _restore = Restore;

    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
        .map_err(|error| format!("could not enter the alternate screen: {error}"))?;
    install_panic_hook();

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|error| format!("could not start the terminal: {error}"))?;

    terminal
        .hide_cursor()
        .map_err(|error| format!("could not hide the cursor: {error}"))?;

    let os = platform::windows_version_string();
    event_loop(&mut terminal, ui, &os)
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ui: &Ui,
    os: &str,
) -> Result<Outcome, String> {
    let control = ui.control();
    let mut selected = 0usize;
    let mut autostart = startup::is_enabled();
    let mut hits = HitRegions::default();
    let mut last_click: Option<(Instant, usize)> = None;

    loop {
        let snapshot = ui.snapshot();
        let last = snapshot.endpoints.len().saturating_sub(1);
        if selected > last {
            selected = last;
        }

        let view = View {
            snapshot: &snapshot,
            selected,
            os,
            autostart,
        };

        terminal
            .draw(|frame| draw(frame, &view, &mut hits))
            .map_err(|error| format!("could not draw the interface: {error}"))?;

        if !event::poll(REFRESH_INTERVAL)
            .map_err(|error| format!("could not poll for input: {error}"))?
        {
            continue;
        }

        match event::read().map_err(|error| format!("could not read input: {error}"))? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(if autostart {
                        Outcome::Tray
                    } else {
                        Outcome::Quit
                    });
                }

                match key.code {
                    KeyCode::Char('q') => {
                        return Ok(if autostart {
                            Outcome::Tray
                        } else {
                            Outcome::Quit
                        });
                    }
                    KeyCode::Char('i') => {
                        let target = !autostart;

                        match startup::set_enabled(target) {
                            Ok(()) => autostart = target,
                            Err(error) => platform::show_error(&format!(
                                "Could not update autostart: {error}"
                            )),
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(last),
                    KeyCode::Enter => {
                        toggle_endpoint(&snapshot, selected, &control);
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => selected = selected.saturating_sub(1),
                MouseEventKind::ScrollDown => selected = (selected + 1).min(last),
                MouseEventKind::Down(MouseButton::Left) => {
                    let point = Position::new(mouse.column, mouse.row);

                    if hits.list.contains(point) {
                        let row =
                            hits.list_offset + usize::from(mouse.row.saturating_sub(hits.list.y));

                        if row < hits.rows {
                            let now = Instant::now();
                            let double_click = last_click.is_some_and(|(time, clicked_row)| {
                                clicked_row == row && now.duration_since(time) <= DOUBLE_CLICK
                            });

                            selected = row;

                            if double_click {
                                last_click = None;
                                toggle_endpoint(&snapshot, row, &control);
                            } else {
                                last_click = Some((now, row));
                            }
                        }
                    } else {
                        last_click = None;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn toggle_endpoint(snapshot: &Snapshot, selected: usize, control: &crate::manager::Control) {
    if let Some(endpoint) = snapshot.endpoints.get(selected) {
        if endpoint.applied {
            control.stop_device(&endpoint.id);
        } else {
            control.start_device(&endpoint.id);
        }
    }
}

fn draw(frame: &mut Frame, view: &View, hits: &mut HitRegions) {
    *hits = HitRegions::default();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(6),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], view.snapshot, view.os, view.autostart);
    draw_endpoints(frame, chunks[1], view.snapshot, view.selected, hits);
    draw_details(frame, chunks[2], view.snapshot, view.selected);
}

fn draw_header(frame: &mut Frame, area: Rect, snapshot: &Snapshot, os: &str, autostart: bool) {
    let autostart_text = if autostart { "on" } else { "off" };
    let quit = if autostart { "close" } else { "quit" };

    let lines = vec![
        Line::from(vec![
            Span::styled("real-rs", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                " {} - reduce audio latency",
                env!("CARGO_PKG_VERSION")
            )),
        ]),
        Line::from(format!(
            "{os}   held {}/{}   failures {}",
            snapshot.held,
            snapshot.endpoints.len(),
            snapshot.open_failed
        )),
        Line::from(Span::styled(
            "↑/↓ select   Enter start/stop",
            Style::default().fg(MUTED),
        )),
        Line::from(Span::styled(
            format!("i autostart: {autostart_text}   q {quit}"),
            Style::default().fg(MUTED),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_endpoints(
    frame: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    selected: usize,
    hits: &mut HitRegions,
) {
    let items: Vec<ListItem> = snapshot
        .endpoints
        .iter()
        .map(|endpoint| {
            let (color, state) = if endpoint.applied {
                (Color::Green, "held")
            } else if endpoint.paused {
                (Color::Red, "paused")
            } else {
                (Color::Red, "not held")
            };

            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(
                    endpoint.name.as_str(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  [{}]  {}", endpoint.role, state)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Endpoints ({}) ", snapshot.endpoints.len())),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");

    let mut state = ListState::default();
    if !snapshot.endpoints.is_empty() {
        state.select(Some(selected));
    }

    frame.render_stateful_widget(list, area, &mut state);

    hits.list = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    hits.list_offset = state.offset();
    hits.rows = snapshot.endpoints.len();
}

fn draw_details(frame: &mut Frame, area: Rect, snapshot: &Snapshot, selected: usize) {
    let lines = match snapshot.endpoints.get(selected) {
        Some(endpoint) => detail_lines(endpoint),
        None => vec![Line::from("No endpoints")],
    };

    let details = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Selected endpoint "),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(details, area);
}

fn detail_lines(endpoint: &EndpointView) -> Vec<Line<'static>> {
    let properties = &endpoint.properties;
    let applied = endpoint.applied;

    vec![
        Line::from(vec![
            Span::styled(
                "● ",
                Style::default().fg(if applied { Color::Green } else { Color::Red }),
            ),
            Span::styled(
                endpoint.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  [{}]", endpoint.role)),
        ]),
        Line::from(format!(
            "{} Hz · {} ch · {}-bit",
            properties.sample_rate, properties.channels, properties.bits_per_sample
        )),
        Line::from(format!(
            "periods: default {:.2} ms ({} frames) · min {:.2} ms ({} frames) · max {:.2} ms ({} frames) · fundamental {} frames",
            properties.milliseconds(properties.default_period),
            properties.default_period,
            properties.milliseconds(properties.minimum_period),
            properties.minimum_period,
            properties.milliseconds(properties.maximum_period),
            properties.maximum_period,
            properties.fundamental_period
        )),
        Line::from(format!("status: {}", endpoint.status)),
        Line::from(Span::styled(
            if applied {
                "applied - press Enter to stop"
            } else if endpoint.paused {
                "paused - press Enter to resume"
            } else {
                "not applied - press Enter to retry"
            },
            Style::default().fg(if applied { Color::Green } else { Color::Yellow }),
        )),
    ]
}

struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            Show
        );
    }
}

fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            Show
        );
        hook(info);
    }));
}
