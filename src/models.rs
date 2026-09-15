//! The slices of the Redfish schema this tool actually reads.
//!
//! Everything is optional. Redfish is a large standard and vendors implement
//! different parts of it: a field that is present on one board is missing on
//! the next, and a sensor that reads fine while the host is powered on reports
//! nothing when it is off. Deserialising into `Option` everywhere means the
//! tool degrades to showing less rather than failing outright.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Status {
    pub state: Option<String>,
    pub health: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct System {
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub bios_version: Option<String>,
    pub power_state: Option<String>,
    pub status: Option<Status>,
    pub processor_summary: Option<ProcessorSummary>,
    pub memory_summary: Option<MemorySummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProcessorSummary {
    pub count: Option<u32>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MemorySummary {
    pub total_system_memory_gi_b: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PowerMetrics {
    pub average_consumed_watts: Option<f64>,
    pub max_consumed_watts: Option<f64>,
    pub min_consumed_watts: Option<f64>,
    pub interval_in_min: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PowerControl {
    pub power_consumed_watts: Option<f64>,
    pub power_capacity_watts: Option<f64>,
    pub power_metrics: Option<PowerMetrics>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Voltage {
    pub name: Option<String>,
    pub reading_volts: Option<f64>,
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Power {
    #[serde(default)]
    pub power_control: Vec<PowerControl>,
    #[serde(default)]
    pub voltages: Vec<Voltage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Fan {
    pub name: Option<String>,
    pub reading: Option<i64>,
    pub reading_units: Option<String>,
    pub lower_threshold_critical: Option<i64>,
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Temperature {
    pub name: Option<String>,
    pub reading_celsius: Option<f64>,
    pub upper_threshold_critical: Option<f64>,
    pub status: Option<Status>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Thermal {
    #[serde(default)]
    pub fans: Vec<Fan>,
    #[serde(default)]
    pub temperatures: Vec<Temperature>,
}

impl Status {
    /// A sensor the BMC reports as `Absent` is not a fault: the host is off, or
    /// that slot is empty. Distinguishing it from an actual failure is the
    /// whole point of showing health at all.
    pub fn is_present(&self) -> bool {
        !matches!(self.state.as_deref(), Some("Absent") | Some("Disabled"))
    }

    pub fn is_ok(&self) -> bool {
        matches!(self.health.as_deref(), Some("OK") | None)
    }
}
