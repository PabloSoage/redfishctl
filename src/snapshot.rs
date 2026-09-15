//! The state the interface draws, and the background task that keeps it fresh.
//!
//! The BMC is polled on a timer in its own task rather than from the draw loop.
//! That matters more than it sounds: a Redfish round trip to a busy BMC can take
//! a second or more, and doing it inside the render path would make the whole
//! interface stutter every time it refreshed. Here the UI always draws whatever
//! the last completed poll left behind, at whatever frame rate it likes.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::client::Client;
use crate::models::{Power, System, Thermal};

/// How many samples of each sensor to keep for the charts. At the default two
/// second interval this is a bit over half an hour of history.
pub const HISTORY: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Temperature,
    Fan,
    Voltage,
}

impl Kind {
    pub fn unit(&self) -> &'static str {
        match self {
            Kind::Temperature => "C",
            Kind::Fan => "RPM",
            Kind::Voltage => "V",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Kind::Temperature => "temperature",
            Kind::Fan => "fan",
            Kind::Voltage => "voltage",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sensor {
    pub name: String,
    /// How Redfish addresses this entry inside its array, needed to patch it.
    pub member_id: Option<String>,
    pub kind: Kind,
    pub reading: Option<f64>,
    pub health: Option<String>,
    pub state: Option<String>,
    pub present: bool,
    /// Critical floor for fans, critical ceiling for temperatures.
    pub lower_critical: Option<f64>,
    pub upper_critical: Option<f64>,
    pub history: VecDeque<f64>,
}

impl Sensor {
    /// Three levels rather than two. `Failing` is the one that earns its keep:
    /// a fan still turning but under its floor, or a temperature inside 10% of
    /// its ceiling. The BMC's own health field only flips once the limit is
    /// crossed, which tells you everything was fine right up to the shutdown.
    pub fn severity(&self) -> Severity {
        if !self.present {
            return Severity::Absent;
        }
        let healthy = matches!(self.health.as_deref(), Some("OK") | None);
        if !healthy {
            return Severity::Bad;
        }
        if let (Some(v), Some(lim)) = (self.reading, self.lower_critical) {
            if lim > 0.0 && v <= lim {
                return Severity::Failing;
            }
        }
        if let (Some(v), Some(lim)) = (self.reading, self.upper_critical) {
            if lim > 0.0 && v >= lim * 0.9 {
                return Severity::Failing;
            }
        }
        Severity::Ok
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Bad,
    Failing,
    Ok,
    Absent,
}

impl Severity {
    pub fn text(&self) -> &'static str {
        match self {
            Severity::Bad => "FAULT",
            Severity::Failing => "warn",
            Severity::Ok => "ok",
            Severity::Absent => "absent",
        }
    }
}

#[derive(Clone, Default)]
pub struct Machine {
    pub power_state: Option<String>,
    pub health: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub bios: Option<String>,
    pub cpu_count: Option<u32>,
    pub cpu_model: Option<String>,
    pub memory_gib: Option<f64>,
}

#[derive(Default)]
pub struct Snapshot {
    pub machine: Machine,
    pub watts: Option<f64>,
    pub watts_avg: Option<f64>,
    pub watts_min: Option<f64>,
    pub watts_max: Option<f64>,
    pub watts_history: VecDeque<f64>,
    pub sensors: Vec<Sensor>,
    /// Empty while everything is fine. Shown in the status bar so a failure
    /// that started ten minutes ago does not look like fresh data.
    pub error: Option<String>,
    pub last_poll: Option<std::time::Instant>,
    pub polls: u64,
    /// True while a poll is in flight. The interface renders this instead of
    /// keeping a sticky message, which used to leave "refreshing" on screen
    /// long after the refresh had finished.
    pub busy: bool,
    pub status: String,
    pub logs: Vec<(String, String, String)>,
}

impl Snapshot {
    pub fn faults(&self) -> Vec<&Sensor> {
        let mut v: Vec<&Sensor> = self
            .sensors
            .iter()
            .filter(|s| matches!(s.severity(), Severity::Bad | Severity::Failing))
            .collect();
        v.sort_by_key(|s| s.severity());
        v
    }
}

pub type Shared = Arc<Mutex<Snapshot>>;

fn push(h: &mut VecDeque<f64>, v: f64) {
    if h.len() == HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

/// Merge a fresh reading into the existing sensor list, preserving history.
/// Sensors are matched by name because Redfish member ids are not stable across
/// firmware versions, while the names on the silkscreen are.
fn merge(dst: &mut Vec<Sensor>, fresh: Vec<Sensor>) {
    for f in fresh {
        if let Some(existing) = dst.iter_mut().find(|s| s.name == f.name) {
            if let Some(v) = f.reading {
                push(&mut existing.history, v);
            }
            existing.member_id = f.member_id;
            existing.reading = f.reading;
            existing.health = f.health;
            existing.state = f.state;
            existing.present = f.present;
            existing.lower_critical = f.lower_critical;
            existing.upper_critical = f.upper_critical;
        } else {
            let mut s = f;
            if let Some(v) = s.reading {
                push(&mut s.history, v);
            }
            dst.push(s);
        }
    }
}

async fn poll_once(c: &Client, shared: &Shared) {
    shared.lock().unwrap().busy = true;
    let mut fresh: Vec<Sensor> = Vec::new();
    let mut err: Option<String> = None;

    // All three at once. A round trip to a busy BMC can take over a second,
    // and chaining them tripled the poll time for no reason: none of them
    // depends on the others.
    let (sys, pw, th) = tokio::join!(
        c.get::<System>("/redfish/v1/Systems/Self"),
        c.get::<Power>("/redfish/v1/Chassis/Self/Power"),
        c.get::<Thermal>("/redfish/v1/Chassis/Self/Thermal"),
    );

    if let Err(e) = &sys {
        err = Some(format!("{e:#}"));
    }

    if let Ok(t) = &th {
        for x in &t.temperatures {
            let st = x.status.as_ref();
            fresh.push(Sensor {
                name: x.name.clone().unwrap_or_else(|| "?".into()),
                member_id: x.member_id.clone(),
                kind: Kind::Temperature,
                reading: x.reading_celsius,
                health: st.and_then(|s| s.health.clone()),
                state: st.and_then(|s| s.state.clone()),
                present: st.is_none_or(|s| s.is_present()),
                lower_critical: None,
                upper_critical: x.upper_threshold_critical,
                history: VecDeque::new(),
            });
        }
        for x in &t.fans {
            let st = x.status.as_ref();
            fresh.push(Sensor {
                name: x.name.clone().unwrap_or_else(|| "?".into()),
                member_id: x.member_id.clone(),
                kind: Kind::Fan,
                reading: x.reading.map(|v| v as f64),
                health: st.and_then(|s| s.health.clone()),
                state: st.and_then(|s| s.state.clone()),
                present: st.is_none_or(|s| s.is_present()),
                lower_critical: x.lower_threshold_critical.map(|v| v as f64),
                upper_critical: None,
                history: VecDeque::new(),
            });
        }
    }
    if let Ok(p) = &pw {
        for x in &p.voltages {
            let st = x.status.as_ref();
            fresh.push(Sensor {
                name: x.name.clone().unwrap_or_else(|| "?".into()),
                member_id: None,
                kind: Kind::Voltage,
                reading: x.reading_volts,
                health: st.and_then(|s| s.health.clone()),
                state: st.and_then(|s| s.state.clone()),
                present: st.is_none_or(|s| s.is_present()),
                lower_critical: None,
                upper_critical: None,
                history: VecDeque::new(),
            });
        }
    }

    let mut g = shared.lock().unwrap();
    if let Ok(s) = sys {
        g.machine = Machine {
            power_state: s.power_state,
            health: s.status.as_ref().and_then(|x| x.health.clone()),
            manufacturer: s.manufacturer,
            model: s.model,
            serial: s.serial_number,
            bios: s.bios_version,
            cpu_count: s.processor_summary.as_ref().and_then(|p| p.count),
            cpu_model: s.processor_summary.as_ref().and_then(|p| p.model.clone()),
            memory_gib: s
                .memory_summary
                .as_ref()
                .and_then(|m| m.total_system_memory_gi_b),
        };
    }
    if let Ok(p) = pw {
        if let Some(pc) = p.power_control.first() {
            g.watts = pc.power_consumed_watts;
            if let Some(w) = pc.power_consumed_watts {
                let mut h = std::mem::take(&mut g.watts_history);
                push(&mut h, w);
                g.watts_history = h;
            }
            if let Some(m) = &pc.power_metrics {
                g.watts_avg = m.average_consumed_watts;
                g.watts_min = m.min_consumed_watts;
                g.watts_max = m.max_consumed_watts;
            }
        }
    }
    let mut sensors = std::mem::take(&mut g.sensors);
    merge(&mut sensors, fresh);
    g.sensors = sensors;
    g.error = err;
    g.last_poll = Some(std::time::Instant::now());
    g.polls += 1;
    g.busy = false;
}

/// Lo que la interfaz puede pedir que se haga. Nada de esto se ejecuta en el
/// bucle de dibujo: alli una espera de un segundo se ve como una interfaz
/// colgada, y pulsar la tecla dos veces encolaba dos esperas.
pub enum Job {
    Logs,
    Reset(&'static str),
    Threshold {
        sensor: String,
        array: &'static str,
        member: String,
        field: &'static str,
        value: f64,
    },
}

fn say(shared: &Shared, msg: impl Into<String>) {
    shared.lock().unwrap().status = msg.into();
}

/// The only place that talks to the BMC.
///
/// On-demand refresh arrives through `Notify` rather than the channel, on
/// purpose: `Notify` COALESCES. Hitting `r` ten times leaves one poll pending
/// instead of queueing ten that the interface would then wait through one
/// after another.
pub fn spawn_worker(
    c: Arc<Client>,
    shared: Shared,
    interval: Duration,
    refresh: Arc<tokio::sync::Notify>,
    mut jobs: tokio::sync::mpsc::UnboundedReceiver<Job>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        poll_once(&c, &shared).await;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => poll_once(&c, &shared).await,
                _ = refresh.notified() => poll_once(&c, &shared).await,
                Some(job) = jobs.recv() => run_job(&c, &shared, job).await,
            }
        }
    })
}

async fn run_job(c: &Client, shared: &Shared, job: Job) {
    match job {
        Job::Logs => load_logs(c, shared).await,
        Job::Reset(kind) => {
            say(shared, format!("{kind}..."));
            let body = serde_json::json!({ "ResetType": kind });
            match c
                .post(
                    "/redfish/v1/Systems/Self/Actions/ComputerSystem.Reset",
                    &body,
                )
                .await
            {
                Ok(()) => {
                    say(shared, format!("{kind} accepted"));
                    poll_once(c, shared).await;
                }
                Err(e) => say(shared, format!("{kind} refused: {e:#}")),
            }
        }
        Job::Threshold {
            sensor,
            array,
            member,
            field,
            value,
        } => {
            say(shared, format!("setting {sensor} {field}..."));
            let body = serde_json::json!({ array: [ { "MemberId": member, field: value } ] });
            match c.patch("/redfish/v1/Chassis/Self/Thermal", &body).await {
                Ok(()) => {
                    say(shared, format!("{sensor} {field} set to {value:.0}"));
                    poll_once(c, shared).await;
                }
                // A refusal is the normal case on many boards, so say it
                // plainly rather than dressing it up as a tool failure.
                Err(e) => say(shared, format!("the BMC refused: {e:#}")),
            }
        }
    }
}

/// Log services vary by vendor, so try the usual places and take the first
/// that answers rather than assuming one layout.
async fn load_logs(c: &Client, shared: &Shared) {
    say(shared, "loading the log...");
    let roots = [
        "/redfish/v1/Systems/Self/LogServices",
        "/redfish/v1/Managers/Self/LogServices",
        "/redfish/v1/Chassis/Self/LogServices",
    ];
    for root in roots {
        let Ok(v) = c.get_value(root).await else {
            continue;
        };
        let members = v
            .get("Members")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        for m in members {
            let Some(id) = m.get("@odata.id").and_then(|s| s.as_str()) else {
                continue;
            };
            let Ok(e) = c.get_value(&format!("{id}/Entries")).await else {
                continue;
            };
            let entries = e
                .get("Members")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            if entries.is_empty() {
                continue;
            }
            let rows: Vec<(String, String, String)> = entries
                .iter()
                .rev()
                .take(200)
                .map(|x| {
                    let get = |k: &str| x.get(k).and_then(|s| s.as_str()).unwrap_or("").to_string();
                    (get("Created"), get("Severity"), get("Message"))
                })
                .collect();
            let n = rows.len();
            let mut g = shared.lock().unwrap();
            g.logs = rows;
            g.status = format!("{n} entries from {id}");
            return;
        }
    }
    say(
        shared,
        "this BMC exposes no readable log service over Redfish",
    );
}
