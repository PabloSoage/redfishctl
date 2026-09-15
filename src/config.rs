//! Where the connection details live.
//!
//! The password is stored in the config file, so the file is created with
//! owner-only permissions on Unix. That is the same trade-off `ipmitool -f`
//! makes: a BMC password is not a secret you can avoid storing if you want
//! unattended monitoring, but it should not be world-readable and it should
//! never reach a shell history or a process listing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Hostname or IP of the BMC, without scheme.
    pub host: String,
    pub user: String,
    pub password: String,
    /// BMCs ship with self-signed certificates. Verification is off by default
    /// because turning it on would make the tool unusable on the hardware it
    /// exists for; set it to true once you have installed a real certificate.
    #[serde(default)]
    pub verify_tls: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    15
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: "admin".into(),
            password: String::new(),
            verify_tls: false,
            timeout_secs: default_timeout(),
        }
    }
}

pub fn path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "bmctl")
        .context("could not determine a config directory for this platform")?;
    Ok(dirs.config_dir().join("config.toml"))
}

pub fn load() -> Result<Config> {
    let p = path()?;
    let text = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "no configuration at {}\n\nRun `bmctl config set --host <ip>` first.",
            p.display()
        )
    })?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("{} is not valid TOML", p.display()))?;
    if cfg.host.is_empty() {
        anyhow::bail!("no host configured. Run `bmctl config set --host <ip>`.");
    }
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<PathBuf> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&p, text).with_context(|| format!("could not write {}", p.display()))?;
    restrict(&p)?;
    Ok(p)
}

#[cfg(unix)]
fn restrict(p: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(p)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(p, perms)?;
    Ok(())
}

/// On Windows the file inherits the user profile's ACL, which is already
/// restricted to the account. There is no portable chmod to apply here.
#[cfg(not(unix))]
fn restrict(_p: &std::path::Path) -> Result<()> {
    Ok(())
}
