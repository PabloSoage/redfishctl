//! redfishctl — control and monitor a server's BMC over Redfish.
//!
//! Redfish is the standard replacement for IPMI's sensor and control surface,
//! and unlike IPMI it is plain HTTPS and JSON. That is why this tool speaks it
//! directly instead of shelling out to `ipmitool`: no external binary, no
//! RMCP+ session, and it works the same on Windows, Linux and macOS.
//!
//! What it deliberately does NOT do is scrape the BMC's web interface. That
//! interface is itself a Redfish client, and scraping it breaks on every
//! firmware update.
//!
//! Running it with no arguments opens the interactive interface, which is the
//! way it is meant to be used. The subcommands exist for scripts; nobody should
//! have to remember a flag to look at a fan.

mod client;
mod commands;
mod config;
mod models;
mod snapshot;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "redfishctl",
    version,
    about = "Control and monitor a server's BMC over Redfish",
    long_about = None
)]
struct Cli {
    /// Seconds between polls in the interactive interface
    #[arg(long, default_value_t = 3.0, global = true)]
    interval: f64,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Connection settings: host, user and password
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Machine summary: power state, model, consumption and health
    Status,
    /// Sensor readings
    Sensors {
        /// Temperatures only
        #[arg(long)]
        temps: bool,
        /// Fans only
        #[arg(long)]
        fans: bool,
        /// Voltages only
        #[arg(long)]
        volts: bool,
        /// Include sensors the BMC reports as absent (usually the host is off)
        #[arg(long)]
        all: bool,
    },
    /// Power control
    Power {
        #[command(subcommand)]
        action: PowerAction,
    },
    /// Follow consumption and temperatures live
    Watch {
        /// Seconds between samples
        #[arg(long, default_value_t = 2.0)]
        interval: f64,
        /// Stop after this many samples (0 = until Ctrl-C)
        #[arg(long, default_value_t = 0)]
        count: u64,
        /// Print comma-separated values, for logging to a file
        #[arg(long)]
        csv: bool,
    },
    /// GET any Redfish path and print the JSON, for exploring
    Raw {
        /// For example /redfish/v1/Chassis/Self/Power
        path: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set host, user and password
    Set {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        user: Option<String>,
        /// Prompt for the password without echoing it
        #[arg(long)]
        password: bool,
        /// Verify the BMC's TLS certificate (off by default: they self-sign)
        #[arg(long)]
        verify_tls: Option<bool>,
    },
    /// Show the current settings, with the password masked
    Show,
    /// Print the path of the config file
    Path,
    /// Check that the BMC answers with these settings
    Test,
}

#[derive(Subcommand)]
enum PowerAction {
    /// Current power state
    Status,
    /// Turn the host on
    On,
    /// Ask the OS to shut down cleanly
    Off,
    /// Cut power immediately, without asking the OS
    ForceOff,
    /// Power cycle, without asking the OS
    Reset,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        // No subcommand: this is the normal way in.
        let cfg = config::load()?;
        return ui::run(cfg, cli.interval).await;
    };
    match command {
        Command::Config { action } => match action {
            ConfigAction::Set {
                host,
                user,
                password,
                verify_tls,
            } => commands::config_set(host, user, password, verify_tls),
            ConfigAction::Show => commands::config_show(),
            ConfigAction::Path => commands::config_path(),
            ConfigAction::Test => commands::config_test().await,
        },
        Command::Status => commands::status().await,
        Command::Sensors {
            temps,
            fans,
            volts,
            all,
        } => commands::sensors(temps, fans, volts, all).await,
        Command::Power { action } => match action {
            PowerAction::Status => commands::power_status().await,
            PowerAction::On => commands::power_action("On").await,
            PowerAction::Off => commands::power_action("GracefulShutdown").await,
            PowerAction::ForceOff => commands::power_action("ForceOff").await,
            PowerAction::Reset => commands::power_action("ForceRestart").await,
        },
        Command::Watch {
            interval,
            count,
            csv,
        } => commands::watch(interval, count, csv).await,
        Command::Raw { path } => commands::raw(&path).await,
    }
}
