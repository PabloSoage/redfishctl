//! What each subcommand does.

use anyhow::{Context, Result};
use serde_json::json;

use crate::client::Client;
use crate::config;
use crate::models::{Power, Status, System, Thermal};

/// These paths are what the Redfish schema prescribes, and `Self` is the
/// conventional singleton id. A BMC that names them differently would need the
/// collections walked first; `bmctl raw /redfish/v1/Chassis` shows the real ids.
const SYSTEM: &str = "/redfish/v1/Systems/Self";
const POWER: &str = "/redfish/v1/Chassis/Self/Power";
const THERMAL: &str = "/redfish/v1/Chassis/Self/Thermal";
const RESET: &str = "/redfish/v1/Systems/Self/Actions/ComputerSystem.Reset";

fn connect() -> Result<Client> {
    Client::new(&config::load()?)
}

// ---------------------------------------------------------------- config ---

pub fn config_set(
    host: Option<String>,
    user: Option<String>,
    password: bool,
    verify_tls: Option<bool>,
) -> Result<()> {
    let mut cfg = config::load().unwrap_or_default();
    if let Some(h) = host {
        // Tolerate a pasted URL: people copy it out of the browser.
        cfg.host = h
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
    }
    if let Some(u) = user {
        cfg.user = u;
    }
    if let Some(v) = verify_tls {
        cfg.verify_tls = v;
    }
    if password || cfg.password.is_empty() {
        // Read from the terminal, never from an argument: anything on the
        // command line lands in the shell history and in a process listing.
        let p = rpassword::prompt_password(format!("Password for {}@{}: ", cfg.user, cfg.host))
            .context("could not read the password")?;
        cfg.password = p;
    }
    let path = config::save(&cfg)?;
    println!("Saved to {}", path.display());
    println!("Check it with `bmctl config test`.");
    Ok(())
}

pub fn config_show() -> Result<()> {
    let cfg = config::load()?;
    println!("  host       {}", cfg.host);
    println!("  user       {}", cfg.user);
    println!(
        "  password   {}",
        if cfg.password.is_empty() {
            "(not set)"
        } else {
            "********"
        }
    );
    println!("  verify_tls {}", cfg.verify_tls);
    println!("  timeout    {} s", cfg.timeout_secs);
    println!("  file       {}", config::path()?.display());
    Ok(())
}

pub fn config_path() -> Result<()> {
    println!("{}", config::path()?.display());
    Ok(())
}

pub async fn config_test() -> Result<()> {
    let cfg = config::load()?;
    let c = Client::new(&cfg)?;
    let root = c.get_value("/redfish/v1").await?;
    let field = |k: &str| {
        root.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string()
    };
    println!("Connected to {}", cfg.host);
    println!("  vendor   {}", field("Vendor"));
    println!("  product  {}", field("Product"));
    println!("  redfish  {}", field("RedfishVersion"));
    Ok(())
}

// ---------------------------------------------------------------- status ---

fn present(s: Option<&Status>) -> bool {
    s.is_none_or(|x| x.is_present())
}

pub async fn status() -> Result<()> {
    let c = connect()?;
    let sys: System = c.get(SYSTEM).await?;

    println!();
    println!(
        "  power        {}",
        sys.power_state.as_deref().unwrap_or("?")
    );
    if let Some(s) = &sys.status {
        let h = s.health.as_deref().or(s.state.as_deref()).unwrap_or("?");
        println!("  health       {h}");
    }
    if let (Some(m), Some(mo)) = (&sys.manufacturer, &sys.model) {
        println!("  machine      {} {}", m.trim(), mo.trim());
    }
    if let Some(sn) = &sys.serial_number {
        println!("  serial       {}", sn.trim());
    }
    if let Some(b) = &sys.bios_version {
        println!("  bios         {}", b.trim());
    }
    if let Some(p) = &sys.processor_summary {
        let model = p.model.as_deref().unwrap_or("").trim().to_string();
        let count = p.count.map(|c| c.to_string()).unwrap_or_default();
        if model.is_empty() {
            println!("  cpus         {count}");
        } else {
            println!("  cpus         {count}  {model}");
        }
    }
    if let Some(m) = &sys.memory_summary {
        if let Some(g) = m.total_system_memory_gi_b {
            println!("  memory       {g:.0} GiB");
        }
    }

    // Power and thermals live on the chassis, not the system. Both belong here
    // so that `status` answers "is it fine?" without needing a second command.
    if let Ok(pw) = c.get::<Power>(POWER).await {
        if let Some(pc) = pw.power_control.first() {
            if let Some(w) = pc.power_consumed_watts {
                let mut line = format!("  consumption  {w:.0} W");
                // The capacity is the PSU rating. Showing the headroom turns a
                // bare number into something you can act on.
                if let Some(cap) = pc.power_capacity_watts {
                    if cap > 0.0 && cap < 100_000.0 {
                        line.push_str(&format!(" of {cap:.0} W"));
                    }
                }
                if let Some(m) = &pc.power_metrics {
                    if let (Some(avg), Some(lo), Some(hi), Some(min)) = (
                        m.average_consumed_watts,
                        m.min_consumed_watts,
                        m.max_consumed_watts,
                        m.interval_in_min,
                    ) {
                        line.push_str(&format!(
                            "   (avg {avg:.0}, {lo:.0}-{hi:.0} over {min} min)"
                        ));
                    }
                }
                println!("{line}");
            }
        }
    }

    if let Ok(th) = c.get::<Thermal>(THERMAL).await {
        let faulty: Vec<String> = th
            .fans
            .iter()
            .filter(|f| {
                f.status
                    .as_ref()
                    .is_some_and(|s| s.is_present() && !s.is_ok())
            })
            .map(|f| {
                format!(
                    "{} at {} RPM",
                    f.name.as_deref().unwrap_or("?"),
                    f.reading.unwrap_or(0)
                )
            })
            .collect();

        let hottest = th
            .temperatures
            .iter()
            .filter(|t| present(t.status.as_ref()))
            .filter_map(|t| Some((t.name.clone()?, t.reading_celsius?)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((name, deg)) = hottest {
            println!("  hottest      {name} {deg:.0} C");
        }

        let live = th
            .fans
            .iter()
            .filter(|f| present(f.status.as_ref()))
            .count();
        if faulty.is_empty() {
            println!("  fans         {live} present, all OK");
        } else {
            println!(
                "  fans         {} FAULTY: {}",
                faulty.len(),
                faulty.join(", ")
            );
        }
    }
    println!();
    Ok(())
}

// --------------------------------------------------------------- sensors ---

/// A sensor the BMC reports as absent is not a fault: the host is off, or that
/// slot is empty. Hiding those by default is what keeps the output readable on
/// a board with 92 sensors of which two thirds are unpopulated.
fn health_of(s: Option<&Status>) -> (String, &'static str) {
    match s {
        None => ("-".to_string(), ""),
        Some(st) => {
            let label = st
                .health
                .as_deref()
                .or(st.state.as_deref())
                .unwrap_or("-")
                .to_string();
            let mark = if st.is_present() && !st.is_ok() {
                "   <-- CHECK"
            } else {
                ""
            };
            (label, mark)
        }
    }
}

pub async fn sensors(temps: bool, fans: bool, volts: bool, all: bool) -> Result<()> {
    let c = connect()?;
    // No flag means everything.
    let (temps, fans, volts) = if !temps && !fans && !volts {
        (true, true, true)
    } else {
        (temps, fans, volts)
    };

    if temps || fans {
        let th: Thermal = c.get(THERMAL).await?;
        if temps {
            println!("\n  temperature              reading   state");
            for t in &th.temperatures {
                if !present(t.status.as_ref()) && !all {
                    continue;
                }
                let (st, mut mark) = health_of(t.status.as_ref());
                let r = t
                    .reading_celsius
                    .map(|v| format!("{v:.0} C"))
                    .unwrap_or_else(|| "-".into());
                // Health only flips once a threshold is crossed. Warning while
                // still inside it gives you time to react instead of a report
                // that everything was fine right up to the shutdown.
                if mark.is_empty() {
                    if let (Some(v), Some(lim)) = (t.reading_celsius, t.upper_threshold_critical) {
                        if lim > 0.0 && v >= lim * 0.9 {
                            mark = "   <-- near limit";
                        }
                    }
                }
                println!(
                    "  {:<22} {:>8}   {}{}",
                    t.name.as_deref().unwrap_or("?"),
                    r,
                    st,
                    mark
                );
            }
        }
        if fans {
            println!("\n  fan                      reading   state");
            for f in &th.fans {
                if !present(f.status.as_ref()) && !all {
                    continue;
                }
                let (st, mut mark) = health_of(f.status.as_ref());
                // A fan still turning but below its critical floor is a fan on
                // the way out. It is worth flagging before it stops, because a
                // stopped one puts every other fan in the box to maximum.
                if mark.is_empty() {
                    if let (Some(v), Some(lim)) = (f.reading, f.lower_threshold_critical) {
                        if lim > 0 && v <= lim {
                            mark = "   <-- below floor";
                        }
                    }
                }
                // Most boards report RPM, some report a duty-cycle percentage.
                // Take the unit from the BMC instead of assuming.
                let unit = f.reading_units.as_deref().unwrap_or("RPM");
                let r = f
                    .reading
                    .map(|v| format!("{v} {unit}"))
                    .unwrap_or_else(|| "-".into());
                println!(
                    "  {:<22} {:>8}   {}{}",
                    f.name.as_deref().unwrap_or("?"),
                    r,
                    st,
                    mark
                );
            }
        }
    }

    if volts {
        let pw: Power = c.get(POWER).await?;
        println!("\n  voltage                  reading   state");
        for v in &pw.voltages {
            if !present(v.status.as_ref()) && !all {
                continue;
            }
            let (st, mark) = health_of(v.status.as_ref());
            let r = v
                .reading_volts
                .map(|x| format!("{x:.2} V"))
                .unwrap_or_else(|| "-".into());
            println!(
                "  {:<22} {:>8}   {}{}",
                v.name.as_deref().unwrap_or("?"),
                r,
                st,
                mark
            );
        }
    }
    println!();
    if !all {
        println!("  Absent sensors hidden; pass --all to show them.\n");
    }
    Ok(())
}

// ----------------------------------------------------------------- power ---

pub async fn power_status() -> Result<()> {
    let c = connect()?;
    let sys: System = c.get(SYSTEM).await?;
    println!("{}", sys.power_state.as_deref().unwrap_or("unknown"));
    Ok(())
}

pub async fn power_action(reset_type: &str) -> Result<()> {
    let cfg = config::load()?;
    let c = Client::new(&cfg)?;
    let sys: System = c.get(SYSTEM).await?;
    let before = sys.power_state.clone().unwrap_or_else(|| "unknown".into());

    // Say what is about to happen and to which machine. A power command typed
    // against the wrong host makes for a bad afternoon.
    println!("{reset_type} -> {} (currently {before})", cfg.host);

    c.post(RESET, &json!({ "ResetType": reset_type })).await?;
    println!("Accepted.");
    Ok(())
}

// ----------------------------------------------------------------- watch ---

pub async fn watch(interval: f64, count: u64, csv: bool) -> Result<()> {
    let c = connect()?;
    let period = std::time::Duration::from_secs_f64(interval.max(0.2));

    if csv {
        println!("unix_time,watts,hottest_c,hottest_name");
    } else {
        println!("\n  time       watts   hottest");
    }

    let mut n = 0u64;
    loop {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let watts = c
            .get::<Power>(POWER)
            .await
            .ok()
            .and_then(|p| p.power_control.first().and_then(|x| x.power_consumed_watts));
        let hot = c.get::<Thermal>(THERMAL).await.ok().and_then(|t| {
            t.temperatures
                .iter()
                .filter(|x| present(x.status.as_ref()))
                .filter_map(|x| Some((x.name.clone()?, x.reading_celsius?)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
        });

        if csv {
            println!(
                "{:.3},{},{},{}",
                now,
                watts.map(|w| format!("{w:.0}")).unwrap_or_default(),
                hot.as_ref()
                    .map(|h| format!("{:.0}", h.1))
                    .unwrap_or_default(),
                hot.as_ref().map(|h| h.0.clone()).unwrap_or_default()
            );
        } else {
            println!(
                "  {}  {:>6}   {}",
                clock(now),
                watts
                    .map(|w| format!("{w:.0} W"))
                    .unwrap_or_else(|| "-".into()),
                hot.map(|h| format!("{} {:.0} C", h.0, h.1))
                    .unwrap_or_else(|| "-".into())
            );
        }

        n += 1;
        if count > 0 && n >= count {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(period) => {}
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

/// Seconds since the epoch as HH:MM:SS, UTC. Not worth a date dependency:
/// `watch` is read in the moment, and the CSV carries the raw epoch anyway.
fn clock(epoch: f64) -> String {
    let secs = epoch as u64 % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

// ------------------------------------------------------------------- raw ---

pub async fn raw(path: &str) -> Result<()> {
    let c = connect()?;
    let p = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let v = c.get_value(&p).await?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
