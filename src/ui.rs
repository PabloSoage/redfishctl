//! The interactive interface.
//!
//! This is the primary way to use the tool: running `redfishctl` with no
//! arguments lands here. The subcommands still exist, because scripts need
//! them, but nobody should have to remember a flag to look at a fan.
//!
//! Everything is one keystroke from the tab bar, the current tab's keys are
//! always on screen, and destructive actions ask first.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{
    Axis, Block, BorderType, Borders, Cell, Chart, Clear, Dataset, GraphType, List, ListItem,
    Paragraph, Row, Table, TableState, Tabs, Wrap,
};

use crate::client::Client;
use crate::config::Config;
use crate::snapshot::{self, Job, Kind, Severity, Shared, Snapshot};

const TABS: [&str; 5] = ["Overview", "Sensors", "Charts", "Power", "Logs"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Temps,
    Fans,
    Volts,
    Problems,
}

impl Filter {
    fn label(&self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Temps => "temperatures",
            Filter::Fans => "fans",
            Filter::Volts => "voltages",
            Filter::Problems => "problems only",
        }
    }
}

/// A question the user has to answer before something irreversible happens.
struct Confirm {
    title: String,
    body: String,
    action: Action,
}

#[derive(Clone)]
enum Action {
    Reset(&'static str),
}

/// Editing a threshold in place. The BMC is the authority on whether it will
/// accept the change: plenty of firmwares expose the field read-only, and the
/// honest thing is to try and show what came back.
struct Edit {
    sensor: String,
    member_id: String,
    field: &'static str,
    array: &'static str,
    current: Option<f64>,
    buffer: String,
}

pub struct App {
    jobs: tokio::sync::mpsc::UnboundedSender<Job>,
    refresh: Arc<tokio::sync::Notify>,
    shared: Shared,
    host: String,
    tab: usize,
    filter: Filter,
    list: TableState,
    /// Sensor selected for the chart tab, by name.
    charted: Option<String>,
    confirm: Option<Confirm>,
    edit: Option<Edit>,
    help: bool,
    logs_asked: bool,
    quit: bool,
}

impl App {
    pub fn new(
        jobs: tokio::sync::mpsc::UnboundedSender<Job>,
        refresh: Arc<tokio::sync::Notify>,
        shared: Shared,
        host: String,
    ) -> Self {
        let mut list = TableState::default();
        list.select(Some(0));
        Self {
            jobs,
            refresh,
            shared,
            host,
            tab: 0,
            filter: Filter::All,
            list,
            charted: None,
            confirm: None,
            edit: None,
            help: false,
            logs_asked: false,
            quit: false,
        }
    }

    fn visible<'a>(&self, snap: &'a Snapshot) -> Vec<&'a snapshot::Sensor> {
        snap.sensors
            .iter()
            .filter(|s| match self.filter {
                Filter::All => s.present,
                Filter::Temps => s.present && s.kind == Kind::Temperature,
                Filter::Fans => s.present && s.kind == Kind::Fan,
                Filter::Volts => s.present && s.kind == Kind::Voltage,
                Filter::Problems => {
                    matches!(s.severity(), Severity::Bad | Severity::Failing)
                }
            })
            .collect()
    }
}

pub async fn run(cfg: Config, interval: f64) -> Result<()> {
    let client = Arc::new(Client::new(&cfg)?);
    let shared: Shared = Arc::new(Mutex::new(Snapshot::default()));
    shared.lock().unwrap().status = "connecting...".into();
    let refresh = Arc::new(tokio::sync::Notify::new());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let worker = snapshot::spawn_worker(
        client,
        shared.clone(),
        Duration::from_secs_f64(interval.max(0.5)),
        refresh.clone(),
        rx,
    );

    let mut term = ratatui::init();
    let mut app = App::new(tx, refresh, shared, cfg.host.clone());
    let mut events = crossterm::event::EventStream::new();
    // Redraw on a timer as well as on input, so the clock and the live values
    // move even when nobody is touching the keyboard.
    let mut tick = tokio::time::interval(Duration::from_millis(250));

    let res = loop {
        if app.quit {
            break Ok(());
        }
        if let Err(e) = term.draw(|f| draw(f, &mut app)) {
            break Err(anyhow::Error::from(e));
        }
        tokio::select! {
            _ = tick.tick() => {}
            Some(Ok(ev)) = events.next() => {
                if let Event::Key(k) = ev {
                    if k.kind == KeyEventKind::Press {
                        on_key(&mut app, k).await;
                    }
                }
            }
        }
    };

    worker.abort();
    ratatui::restore();
    res
}

async fn on_key(app: &mut App, k: KeyEvent) {
    if app.edit.is_some() {
        match k.code {
            KeyCode::Esc => {
                app.edit = None;
                say(app, "cancelled");
            }
            KeyCode::Backspace => {
                if let Some(e) = app.edit.as_mut() {
                    e.buffer.pop();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if let Some(e) = app.edit.as_mut() {
                    if e.buffer.len() < 12 {
                        e.buffer.push(c);
                    }
                }
            }
            KeyCode::Enter => apply_edit(app),
            _ => {}
        }
        return;
    }

    // A pending question swallows every other key: no accidental power-offs
    // because a keystroke was queued behind the dialog.
    if let Some(c) = &app.confirm {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let action = c.action.clone();
                app.confirm = None;
                match action {
                    Action::Reset(kind) => {
                        let _ = app.jobs.send(Job::Reset(kind));
                    }
                }
            }
            _ => {
                app.confirm = None;
                say(app, "cancelled");
            }
        }
        return;
    }

    if app.help {
        app.help = false;
        return;
    }

    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('?') | KeyCode::Char('h') => app.help = true,
        KeyCode::Tab | KeyCode::Right => app.tab = (app.tab + 1) % TABS.len(),
        KeyCode::BackTab | KeyCode::Left => app.tab = (app.tab + TABS.len() - 1) % TABS.len(),
        KeyCode::Char(d @ '1'..='5') => app.tab = d as usize - '1' as usize,
        // Notify, do not wait. `Notify` coalesces, so leaning on the key
        // queues nothing.
        KeyCode::Char('r') => app.refresh.notify_one(),
        KeyCode::Up => move_sel(app, -1),
        KeyCode::Down => move_sel(app, 1),
        KeyCode::Home => app.list.select(Some(0)),
        _ => {}
    }

    match app.tab {
        1 => match k.code {
            KeyCode::Char('a') => set_filter(app, Filter::All),
            KeyCode::Char('t') => set_filter(app, Filter::Temps),
            KeyCode::Char('f') => set_filter(app, Filter::Fans),
            KeyCode::Char('v') => set_filter(app, Filter::Volts),
            KeyCode::Char('p') => set_filter(app, Filter::Problems),
            KeyCode::Char('e') => start_edit(app),
            KeyCode::Enter => {
                // Send the highlighted sensor to the chart tab and go there.
                let snap = app.shared.lock().unwrap();
                let vis = app.visible(&snap);
                if let Some(i) = app.list.selected() {
                    if let Some(s) = vis.get(i) {
                        app.charted = Some(s.name.clone());
                    }
                }
                drop(snap);
                app.tab = 2;
            }
            _ => {}
        },
        3 => {
            let act = match k.code {
                KeyCode::Char('o') => Some(("On", "Turn the host on.")),
                KeyCode::Char('s') => Some((
                    "GracefulShutdown",
                    "Ask the operating system to shut down cleanly.",
                )),
                KeyCode::Char('F') => Some((
                    "ForceOff",
                    "Cut power immediately. The OS is not asked and unsaved work is lost.",
                )),
                KeyCode::Char('R') => Some((
                    "ForceRestart",
                    "Power cycle immediately. The OS is not asked.",
                )),
                _ => None,
            };
            if let Some((kind, what)) = act {
                let state = app
                    .shared
                    .lock()
                    .unwrap()
                    .machine
                    .power_state
                    .clone()
                    .unwrap_or_else(|| "unknown".into());
                app.confirm = Some(Confirm {
                    title: format!("{kind} on {}", app.host),
                    body: format!("{what}\n\nThe host is currently {state}.\n\ny to confirm, any other key to cancel."),
                    action: Action::Reset(kind),
                });
            }
        }
        // First visit loads it, `l` reloads it. Either way the worker does the
        // fetching, so opening the tab never freezes the interface.
        4 if k.code == KeyCode::Char('l') || !app.logs_asked => {
            app.logs_asked = true;
            let _ = app.jobs.send(Job::Logs);
        }
        _ => {}
    }
}

/// Open the editor on the highlighted sensor. Only fans and temperatures have
/// a threshold worth touching, and only if the BMC gave us a member id.
fn start_edit(app: &mut App) {
    let shared = app.shared.clone();
    let snap = shared.lock().unwrap();
    let vis = app.visible(&snap);
    let Some(s) = app.list.selected().and_then(|i| vis.get(i)).copied() else {
        return;
    };
    let (array, field, current) = match s.kind {
        Kind::Fan => ("Fans", "LowerThresholdCritical", s.lower_critical),
        Kind::Temperature => ("Temperatures", "UpperThresholdCritical", s.upper_critical),
        Kind::Voltage => {
            drop(snap);
            say(app, "voltage thresholds are not editable here");
            return;
        }
    };
    let Some(id) = s.member_id.clone() else {
        drop(snap);
        say(app, "this BMC did not give that sensor a member id");
        return;
    };
    let edit = Edit {
        sensor: s.name.clone(),
        member_id: id,
        field,
        array,
        current,
        buffer: current.map(|v| format!("{v:.0}")).unwrap_or_default(),
    };
    drop(snap);
    app.edit = Some(edit);
}

fn apply_edit(app: &mut App) {
    let Some(e) = app.edit.take() else { return };
    let Ok(v) = e.buffer.trim().parse::<f64>() else {
        say(app, format!("{} is not a number", e.buffer));
        return;
    };
    // Handed to the worker. The PATCH and the re-poll that follows it both talk
    // to the BMC, and neither belongs in the key handler.
    let _ = app.jobs.send(Job::Threshold {
        sensor: e.sensor,
        array: e.array,
        member: e.member_id,
        field: e.field,
        value: v,
    });
}

fn say(app: &App, msg: impl Into<String>) {
    app.shared.lock().unwrap().status = msg.into();
}

fn set_filter(app: &mut App, f: Filter) {
    app.filter = f;
    app.list.select(Some(0));
}

fn move_sel(app: &mut App, delta: i32) {
    let n = {
        let snap = app.shared.lock().unwrap();
        app.visible(&snap).len()
    };
    if n == 0 {
        return;
    }
    let cur = app.list.selected().unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(n as i32);
    app.list.select(Some(next as usize));
}

// ------------------------------------------------------------------ draw ---

fn colour(s: Severity) -> Color {
    match s {
        Severity::Bad => Color::Red,
        Severity::Failing => Color::Yellow,
        Severity::Ok => Color::Green,
        Severity::Absent => Color::DarkGray,
    }
}

/// Takes an owned title so callers can pass a `format!` directly: with a
/// borrowed `&str` the temporary dies before the block is rendered.
fn frame(title: impl std::fmt::Display) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
}

fn draw(f: &mut Frame, app: &mut App) {
    // Clonar el Arc antes de bloquear: si el guard sale de `app.shared`, toma
    // prestado `app` entero y ya no se le puede pasar como mutable a las
    // funciones de dibujo.
    let shared = app.shared.clone();
    let snap = shared.lock().unwrap();
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(area);

    let titles: Vec<Line> = TABS
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {} {} ", i + 1, t)))
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.tab)
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .block(frame(format!("redfishctl — {}", app.host))),
        rows[0],
    );

    match app.tab {
        0 => draw_overview(f, rows[1], &snap),
        1 => draw_sensors(f, rows[1], app, &snap),
        2 => draw_charts(f, rows[1], app, &snap),
        3 => draw_power(f, rows[1], &snap),
        _ => draw_logs(f, rows[1], &snap),
    }

    let keys = match app.tab {
        1 => "a all  t temps  f fans  v volts  p problems  enter chart  e edit threshold",
        2 => "enter a sensor on the Sensors tab to chart it",
        3 => "o on  s shutdown  F force-off  R reset",
        4 => "l reload",
        _ => "",
    };
    let age = snap
        .last_poll
        .map(|t| format!("{:.0}s ago", t.elapsed().as_secs_f64()))
        .unwrap_or_else(|| "never".into());
    let left = if let Some(e) = &snap.error {
        format!(" BMC ERROR: {e}")
    } else {
        format!(" {keys}")
    };
    let what = if snap.busy {
        "polling...".to_string()
    } else {
        snap.status.clone()
    };
    let right = format!("{what} | polled {age} | q quit  ? help ");
    let bar = Line::from(vec![
        Span::styled(
            left,
            Style::default().fg(if snap.error.is_some() {
                Color::Red
            } else {
                Color::Gray
            }),
        ),
        Span::raw(" "),
    ]);
    f.render_widget(Paragraph::new(bar), rows[2]);
    let w = rows[2].width as usize;
    if right.len() < w {
        let mut r = rows[2];
        r.x += (w - right.len()) as u16;
        r.width = right.len() as u16;
        f.render_widget(
            Paragraph::new(Span::styled(right, Style::default().fg(Color::DarkGray))),
            r,
        );
    }

    if let Some(e) = &app.edit {
        let now = e
            .current
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "not set".into());
        popup(
            f,
            area,
            &format!("{} — {}", e.sensor, e.field),
            &format!(
                "Currently {now}.

New value: {}_

Enter to send, Esc to cancel.

Many BMCs expose this read-only; if yours does, it will
say so and nothing changes.",
                e.buffer
            ),
            Color::Yellow,
        );
    } else if let Some(c) = &app.confirm {
        popup(f, area, &c.title, &c.body, Color::Red);
    } else if app.help {
        popup(
            f,
            area,
            "Keys",
            "1-5 or Tab      switch tab\n\
             up / down       move through the list\n\
             enter           chart the highlighted sensor\n\
             a t f v p       filter: all, temps, fans, volts, problems\n\
             r               poll now\n\
             o s F R         power: on, shutdown, force-off, reset\n\
             l               reload the log\n\
             q or Esc        quit\n\n\
             Absent sensors are hidden: on a board with 92 of them, two thirds\n\
             report nothing while the host is off.",
            Color::Cyan,
        );
    }
}

fn popup(f: &mut Frame, area: Rect, title: &str, body: &str, col: Color) {
    // No minimum width: forcing one wider than the terminal would push the
    // popup off the side of a narrow window.
    let w = area.width.min(74);
    let h = (body.lines().count() as u16 + 4).min(area.height);
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(frame(title).border_style(Style::default().fg(col))),
        r,
    );
}

fn kv(k: &str, v: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {k:<14}"), Style::default().fg(Color::DarkGray)),
        Span::raw(v),
    ])
}

fn draw_overview(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    let m = &snap.machine;

    let mut lines = vec![
        kv("power", m.power_state.clone().unwrap_or_else(|| "?".into())),
        kv("health", m.health.clone().unwrap_or_else(|| "?".into())),
    ];
    if let (Some(a), Some(b)) = (&m.manufacturer, &m.model) {
        lines.push(kv("machine", format!("{} {}", a.trim(), b.trim())));
    }
    if let Some(s) = &m.serial {
        lines.push(kv("serial", s.trim().into()));
    }
    if let Some(b) = &m.bios {
        lines.push(kv("bios", b.trim().into()));
    }
    if let Some(c) = m.cpu_count {
        let model = m.cpu_model.clone().unwrap_or_default();
        lines.push(kv("cpus", format!("{c}  {}", model.trim())));
    }
    if let Some(g) = m.memory_gib {
        lines.push(kv("memory", format!("{g:.0} GiB")));
    }
    lines.push(Line::raw(""));
    if let Some(w) = snap.watts {
        lines.push(kv("consumption", format!("{w:.0} W")));
    }
    if let (Some(a), Some(lo), Some(hi)) = (snap.watts_avg, snap.watts_min, snap.watts_max) {
        lines.push(kv("reported", format!("avg {a:.0}, {lo:.0}-{hi:.0} W")));
    }
    f.render_widget(Paragraph::new(lines).block(frame("Machine")), cols[0]);

    // Anything wrong goes in its own panel rather than being a colour in a long
    // list, because the point of the overview is to answer "is it fine?".
    let faults = snap.faults();
    let items: Vec<ListItem> = if faults.is_empty() {
        vec![ListItem::new(Line::styled(
            "  nothing to report",
            Style::default().fg(Color::Green),
        ))]
    } else {
        faults
            .iter()
            .map(|s| {
                let sev = s.severity();
                let val = s
                    .reading
                    .map(|v| format!("{v:.0} {}", s.kind.unit()))
                    .unwrap_or_else(|| "-".into());
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {:<7}", sev.text()),
                        Style::default().fg(colour(sev)),
                    ),
                    Span::raw(format!("{:<20} {val}", s.name)),
                ]))
            })
            .collect()
    };
    f.render_widget(
        List::new(items).block(frame(format!("Attention ({})", faults.len()))),
        cols[1],
    );
}

fn draw_sensors(f: &mut Frame, area: Rect, app: &mut App, snap: &Snapshot) {
    let vis = app.visible(snap);
    let rows: Vec<Row> = vis
        .iter()
        .map(|s| {
            let sev = s.severity();
            let val = s
                .reading
                .map(|v| {
                    if s.kind == Kind::Voltage {
                        format!("{v:.2} {}", s.kind.unit())
                    } else {
                        format!("{v:.0} {}", s.kind.unit())
                    }
                })
                .unwrap_or_else(|| "-".into());
            let limit = match (s.lower_critical, s.upper_critical) {
                (Some(l), _) if l > 0.0 => format!("min {l:.0}"),
                (_, Some(u)) if u > 0.0 => format!("max {u:.0}"),
                _ => String::new(),
            };
            Row::new(vec![
                Cell::from(s.name.clone()),
                Cell::from(s.kind.label()),
                Cell::from(val),
                Cell::from(limit),
                Cell::from(Span::styled(sev.text(), Style::default().fg(colour(sev)))),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(13),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(7),
        ],
    )
    .header(
        Row::new(vec!["sensor", "kind", "reading", "limit", "state"])
            .style(Style::default().fg(Color::DarkGray)),
    )
    .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)))
    .block(frame(format!(
        "Sensors — {} ({} shown)",
        app.filter.label(),
        vis.len()
    )));

    let mut st = app.list;
    f.render_stateful_widget(table, area, &mut st);
    app.list = st;
}

fn draw_charts(f: &mut Frame, area: Rect, app: &App, snap: &Snapshot) {
    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    chart(
        f,
        rows[0],
        "Power draw (W)",
        &snap.watts_history,
        Color::Cyan,
    );

    match app
        .charted
        .as_ref()
        .and_then(|n| snap.sensors.iter().find(|s| &s.name == n))
    {
        Some(s) => chart(
            f,
            rows[1],
            &format!("{} ({})", s.name, s.kind.unit()),
            &s.history,
            colour(s.severity()),
        ),
        None => f.render_widget(
            Paragraph::new("\n  Pick a sensor on the Sensors tab and press enter.")
                .block(frame("Sensor")),
            rows[1],
        ),
    }
}

fn chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    hist: &std::collections::VecDeque<f64>,
    col: Color,
) {
    if hist.len() < 2 {
        f.render_widget(
            Paragraph::new("\n  collecting...").block(frame(title)),
            area,
        );
        return;
    }
    let pts: Vec<(f64, f64)> = hist
        .iter()
        .enumerate()
        .map(|(i, v)| (i as f64, *v))
        .collect();
    let lo = hist.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = hist.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // A flat line at the top of an auto-scaled axis looks like a problem. Pad
    // the range so a steady reading reads as steady.
    let pad = ((hi - lo) * 0.1).max(1.0);
    let (lo, hi) = (lo - pad, hi + pad);

    let ds = vec![Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(col))
        .data(&pts)];

    f.render_widget(
        Chart::new(ds)
            .block(frame(title))
            .x_axis(Axis::default().bounds([0.0, pts.len() as f64 - 1.0]))
            .y_axis(
                Axis::default()
                    .bounds([lo, hi])
                    .labels(vec![format!("{lo:.0}"), format!("{hi:.0}")])
                    .style(Style::default().fg(Color::DarkGray)),
            ),
        area,
    );
}

fn draw_power(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let state = snap
        .machine
        .power_state
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let on = state.eq_ignore_ascii_case("on");
    let body = vec![
        Line::from(vec![
            Span::styled("  The host is ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.clone(),
                Style::default()
                    .fg(if on { Color::Green } else { Color::DarkGray })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  o  ", Style::default().fg(Color::Cyan)),
            Span::raw("On                 power the host up"),
        ]),
        Line::from(vec![
            Span::styled("  s  ", Style::default().fg(Color::Cyan)),
            Span::raw("GracefulShutdown   ask the OS to shut down"),
        ]),
        Line::from(vec![
            Span::styled("  F  ", Style::default().fg(Color::Red)),
            Span::raw("ForceOff           cut power, OS not asked"),
        ]),
        Line::from(vec![
            Span::styled("  R  ", Style::default().fg(Color::Red)),
            Span::raw("ForceRestart       power cycle, OS not asked"),
        ]),
        Line::raw(""),
        Line::styled(
            "  Every one of these asks for confirmation first.",
            Style::default().fg(Color::DarkGray),
        ),
    ];
    f.render_widget(Paragraph::new(body).block(frame("Power")), area);
}

fn draw_logs(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let items: Vec<ListItem> = if snap.logs.is_empty() {
        vec![ListItem::new("  press l to load")]
    } else {
        snap.logs
            .iter()
            .map(|(when, sev, msg)| {
                let c = match sev.as_str() {
                    "Critical" | "Fatal" => Color::Red,
                    "Warning" => Color::Yellow,
                    _ => Color::Gray,
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {:<22}", when),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(format!("{:<9}", sev), Style::default().fg(c)),
                    Span::raw(msg.clone()),
                ]))
            })
            .collect()
    };
    f.render_widget(
        List::new(items).block(frame(format!("Log ({} entries)", snap.logs.len()))),
        area,
    );
}
