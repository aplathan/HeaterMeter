//! Shared application state.

use crate::config::Config;
use crate::device::Device;
use crate::storage::Storage;
use lm_core::Settings;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

pub struct App {
    config: RwLock<Config>,
    pub config_path: PathBuf,
    pub device: Arc<Device>,
    pub storage: Storage,
    pub http: reqwest::Client,
    pub started: Instant,
    /// Held while the AVR is being flashed
    pub flash_lock: tokio::sync::Mutex<()>,
    pub uuid: String,
    pub wifi: crate::wifi::WifiState,
    /// Temperature units requested by the boot config, applied once HeaterMeter is configured
    pending_units: std::sync::Mutex<Option<char>>,
}

pub type AppRef = Arc<App>;

impl App {
    pub fn new(config: Config, config_path: PathBuf, device: Arc<Device>, storage: Storage, uuid: String) -> App {
        // rustls needs a process-wide crypto provider (ring) before any TLS client is built
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(concat!("linkmeterd/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        App {
            config: RwLock::new(config),
            config_path,
            device,
            storage,
            http,
            started: Instant::now(),
            flash_lock: tokio::sync::Mutex::new(()),
            uuid,
            wifi: Default::default(),
            pending_units: Default::default(),
        }
    }

    pub fn set_pending_units(&self, u: Option<char>) {
        *self.pending_units.lock().unwrap_or_else(|e| e.into_inner()) = u;
    }

    pub fn take_pending_units(&self) -> Option<char> {
        self.pending_units.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Modify, persist and propagate the configuration.
    pub fn update_config<R>(&self, f: impl FnOnce(&mut Config) -> R) -> std::io::Result<R> {
        let (r, cfg) = {
            let mut g = self.config.write().unwrap_or_else(|e| e.into_inner());
            let r = f(&mut g);
            (r, g.clone())
        };
        cfg.save(&self.config_path)?;
        self.device.set_settings(settings_of(&cfg));
        Ok(r)
    }
}

pub fn settings_of(cfg: &Config) -> Settings {
    Settings { lidtrack_enabled: cfg.daemon.lidtrack_enabled }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Random hex string from the OS RNG.
pub fn random_hex(bytes: usize) -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = vec![0u8; bytes];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "heatermeter".into())
}

/// Primary IPv4 address (first non-loopback, preferring wired/wifi).
pub fn primary_ip() -> Option<String> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    let mut v4: Vec<_> = addrs.into_iter().filter(|a| !a.is_loopback() && a.ip().is_ipv4()).collect();
    v4.sort_by_key(|a| {
        let n = a.name.as_str();
        if n.starts_with("eth") || n.starts_with("en") {
            0
        } else if n.starts_with("wlan") || n.starts_with("wl") {
            1
        } else {
            2
        }
    });
    v4.first().map(|a| a.ip().to_string())
}
