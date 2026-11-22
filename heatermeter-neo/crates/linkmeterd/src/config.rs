//! Daemon configuration (`/etc/linkmeter/config.toml`), replacing the
//! OpenWrt UCI file `/etc/config/linkmeter`.

use lm_core::plugins::RampParams;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub api: ApiConfig,
    pub auth: AuthConfig,
    pub ramp: RampParams,
    pub alarms: AlarmsConfig,
    pub smtp: SmtpConfig,
    pub firmware: FirmwareConfig,
    pub ui: UiConfig,
    pub wifi: WifiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WifiConfig {
    /// Manage wifi via NetworkManager (`nmcli`)
    pub enabled: bool,
    pub interface: String,
    /// Start an access point if no network is connected after boot
    pub fallback_ap: bool,
    pub ap_ssid: String,
    /// Empty = open network
    pub ap_password: String,
}

impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            enabled: cfg!(target_os = "linux"),
            interface: "wlan0".into(),
            fallback_ap: true,
            ap_ssid: "heatermeter".into(),
            ap_password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DaemonConfig {
    /// Serial device; `auto` picks /dev/serial0, /dev/ttyS0 or /dev/ttyAMA0.
    /// `sim` runs the built-in HeaterMeter simulator.
    pub serial_device: String,
    pub serial_baud: u32,
    /// Write in 8-byte chunks (Pi mini-UART FIFO workaround)
    pub serial_chunked: bool,
    /// Optional file to log raw serial traffic to
    pub serial_log: Option<PathBuf>,
    /// HTTP listen address
    pub listen: String,
    pub data_dir: PathBuf,
    pub lidtrack_enabled: bool,
    /// Start a new session if the last sample is older than this (hours)
    pub session_gap_hours: u32,
    /// Advertise via SSDP (UPnP discovery)
    pub ssdp: bool,
    /// Simulator speed-up factor when `serial_device = "sim"`
    pub sim_speed: f64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            serial_device: "auto".into(),
            serial_baud: 38400,
            serial_chunked: true,
            serial_log: None,
            listen: "0.0.0.0:80".into(),
            data_dir: "/var/lib/linkmeter".into(),
            lidtrack_enabled: false,
            session_gap_hours: 6,
            ssdp: true,
            sim_speed: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    pub disabled: bool,
    pub allowcors: bool,
    pub key: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig { disabled: false, allowcors: true, key: String::new() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AuthConfig {
    /// Argon2 PHC string of the admin password; empty = not set yet
    pub password_hash: String,
    /// Allow read-only (guest) access to the dashboard without logging in
    pub guest_read: Option<bool>,
}

impl AuthConfig {
    pub fn guest_read(&self) -> bool {
        self.guest_read.unwrap_or(true)
    }
}

/// What to do when an alarm rings (`ra` in the old UCI config).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RingAction {
    #[default]
    Ring,
    Silence,
    Disable,
}

/// Per-alarm actions, keyed `"0L"`, `"0H"`, ... `"3H"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlarmAction {
    pub email: bool,
    pub sms: bool,
    pub push: bool,
    /// Change the pit setpoint to this value when ringing
    pub setpoint: Option<String>,
    pub action: RingAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlarmsConfig {
    pub actions: BTreeMap<String, AlarmAction>,
    pub email: EmailConfig,
    pub sms: SmsConfig,
    pub push: PushConfig,
    /// Directory with user scripts `alarm-all`, `alarm-0L`, ...
    pub scripts_dir: PathBuf,
}

impl Default for AlarmsConfig {
    fn default() -> Self {
        AlarmsConfig {
            actions: BTreeMap::new(),
            email: EmailConfig::default(),
            sms: SmsConfig::default(),
            push: PushConfig::default(),
            scripts_dir: "/etc/linkmeter/scripts".into(),
        }
    }
}

pub const DEFAULT_EMAIL_BODY: &str = "$pn Alarm ($al_type) $al_prep threshold $al_thresh, currently $pcurr.

$pn0: $pcurr0
$pn1: $pcurr1
$pn2: $pcurr2
$pn3: $pcurr3
--
http://$ip/
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmailConfig {
    pub to_name: String,
    pub to_address: String,
    pub from_name: String,
    pub subject: String,
    pub body: String,
}

impl Default for EmailConfig {
    fn default() -> Self {
        EmailConfig {
            to_name: String::new(),
            to_address: String::new(),
            from_name: "HeaterMeter".into(),
            subject: "$pn $al_prep $al_thresh".into(),
            body: DEFAULT_EMAIL_BODY.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SmsConfig {
    /// `digits@carrier-gateway`
    pub to_address: String,
    pub message: String,
}

impl Default for SmsConfig {
    fn default() -> Self {
        SmsConfig { to_address: String::new(), message: "HeaterMeter Alert: $pn is $pcurr".into() }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PushProvider {
    #[default]
    None,
    Pushover,
    Pushbullet,
    Ntfy,
    Webhook,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PushConfig {
    pub provider: PushProvider,
    pub title: String,
    pub message: String,
    pub pushbullet_key: String,
    pub pushover_user: String,
    pub pushover_token: String,
    pub pushover_sound: String,
    /// ntfy topic URL, e.g. `https://ntfy.sh/my-grill`
    pub ntfy_url: String,
    pub ntfy_token: String,
    /// Webhook URL receiving a JSON POST
    pub webhook_url: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        PushConfig {
            provider: PushProvider::None,
            title: "HeaterMeter $pn".into(),
            message: "$pn alarm: $al_prep $al_thresh, currently $pcurr".into(),
            pushbullet_key: String::new(),
            pushover_user: String::new(),
            pushover_token: String::new(),
            pushover_sound: "pushover".into(),
            ntfy_url: String::new(),
            ntfy_token: String::new(),
            webhook_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    #[default]
    Starttls,
    Tls,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: String,
    pub from_address: String,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        SmtpConfig {
            host: String::new(),
            port: 587,
            security: SmtpSecurity::Starttls,
            username: String::new(),
            password: String::new(),
            from_address: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FirmwareConfig {
    /// Enable AVR flashing over SPI (Raspberry Pi with HeaterMeter board)
    pub enabled: bool,
    pub avrdude: String,
    pub spi_device: String,
    /// GPIO chip and line used for the AVR reset pin (BCM 25)
    pub gpio_chip: String,
    pub reset_line: u32,
    /// Bundled firmware image
    pub bundled_hex: PathBuf,
    /// Reflash a blank chip automatically if HeaterMeter does not respond
    pub auto_recover: bool,
}

impl Default for FirmwareConfig {
    fn default() -> Self {
        FirmwareConfig {
            enabled: cfg!(target_os = "linux"),
            avrdude: "avrdude".into(),
            spi_device: "/dev/spidev0.0".into(),
            gpio_chip: "auto".into(),
            reset_line: 25,
            bundled_hex: "/usr/share/linkmeter/hm.hex".into(),
            auto_recover: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    /// Extra CSS injected into the web UI
    pub user_css: String,
}

impl Config {
    pub fn load(path: &Path) -> anyhow_lite::Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s).map_err(|e| anyhow_lite::err(format!("{}: {e}", path.display())))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(anyhow_lite::err(format!("{}: {e}", path.display()))),
        }
    }

    /// Atomically write the configuration.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, path)
    }

    pub fn action(&self, probe: usize, kind: char) -> AlarmAction {
        self.alarms.actions.get(&format!("{probe}{kind}")).cloned().unwrap_or_default()
    }
}

/// Tiny error helper to avoid an extra dependency.
pub mod anyhow_lite {
    pub type Error = Box<dyn std::error::Error + Send + Sync>;
    pub type Result<T> = std::result::Result<T, Error>;
    pub fn err(msg: impl Into<String>) -> Error {
        msg.into().into()
    }
}

/// Import settings from an OpenWrt UCI file (`/etc/config/linkmeter`).
pub fn import_uci(text: &str, mut cfg: Config) -> Config {
    let mut section = String::new();
    let mut opts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let mut parts = line.splitn(3, char::is_whitespace);
        match (parts.next(), parts.next(), parts.next()) {
            (Some("config"), Some(_ty), Some(name)) => section = unquote(name),
            (Some("config"), Some(ty), None) => section = ty.to_string(),
            (Some("option"), Some(k), Some(v)) => {
                opts.entry(section.clone()).or_default().insert(k.to_string(), unquote(v));
            }
            _ => {}
        }
    }
    let get = |s: &str, k: &str| opts.get(s).and_then(|m| m.get(k)).cloned();
    let flag = |s: &str, k: &str| get(s, k).map(|v| v == "1");

    if let Some(v) = get("daemon", "serial_device") {
        cfg.daemon.serial_device = if v.starts_with("/dev/ttyS1") { "auto".into() } else { v };
    }
    if let Some(v) = get("daemon", "serial_baud").and_then(|v| v.parse().ok()) {
        cfg.daemon.serial_baud = v;
    }
    if let Some(v) = flag("daemon", "lidtrack_enabled") {
        cfg.daemon.lidtrack_enabled = v;
    }
    if let Some(v) = flag("api", "disabled") {
        cfg.api.disabled = v;
    }
    if let Some(v) = flag("api", "allowcors") {
        cfg.api.allowcors = v;
    }
    if let Some(v) = get("api", "key") {
        cfg.api.key = v;
    }
    if let Some(v) = flag("ramp", "enabled") {
        cfg.ramp.enabled = v;
    }
    if let Some(v) = get("ramp", "watch") {
        cfg.ramp.watch = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    }
    if let Some(v) = get("ramp", "trigger").and_then(|v| v.parse().ok()) {
        cfg.ramp.trigger = v;
    }
    if let Some(v) = get("ramp", "target").and_then(|v| v.parse().ok()) {
        cfg.ramp.target = v;
    }
    cfg.ramp.start_setpoint = get("ramp", "startsetpoint").and_then(|v| v.parse().ok());

    for p in 0..4 {
        for k in ['L', 'H'] {
            let mut a = cfg.action(p, k);
            if let Some(v) = flag("alarms", &format!("email{k}{p}")) {
                a.email = v;
            }
            if let Some(v) = flag("alarms", &format!("sms{k}{p}")) {
                a.sms = v;
            }
            if let Some(v) = flag("alarms", &format!("push{k}{p}")) {
                a.push = v;
            }
            a.setpoint = get("alarms", &format!("sp{k}{p}")).filter(|v| !v.is_empty() && v != "0");
            a.action = match get("alarms", &format!("ra{k}{p}")).as_deref() {
                Some("1") => RingAction::Silence,
                Some("2") => RingAction::Disable,
                _ => RingAction::Ring,
            };
            if a != AlarmAction::default() {
                cfg.alarms.actions.insert(format!("{p}{k}"), a);
            }
        }
    }
    let e = &mut cfg.alarms.email;
    if let Some(v) = get("alarms_email", "toname") {
        e.to_name = v;
    }
    if let Some(v) = get("alarms_email", "toaddress") {
        e.to_address = v;
    }
    if let Some(v) = get("alarms_email", "fromname") {
        e.from_name = v;
    }
    if let Some(v) = get("alarms_email", "subject") {
        e.subject = v;
    }
    if let Some(v) = get("alarms_sms", "toaddress") {
        cfg.alarms.sms.to_address = v;
    }
    if let Some(v) = get("alarms_sms", "message") {
        cfg.alarms.sms.message = v;
    }
    let p = &mut cfg.alarms.push;
    p.provider = match get("alarms_push", "provider").as_deref() {
        Some("pushbullet") => PushProvider::Pushbullet,
        Some("pushover") => PushProvider::Pushover,
        _ => p.provider,
    };
    if let Some(v) = get("alarms_push", "pushbulletkey") {
        p.pushbullet_key = v;
    }
    if let Some(v) = get("alarms_push", "pushbullettitle") {
        p.title = v;
    }
    if let Some(v) = get("alarms_push", "pushoveruser") {
        p.pushover_user = v;
    }
    if let Some(v) = get("alarms_push", "pushovertoken") {
        p.pushover_token = v;
    }
    if let Some(v) = get("alarms_push", "pushoversound") {
        p.pushover_sound = v;
    }
    if let Some(v) = get("alarms_push", "message") {
        p.message = v;
    }
    cfg
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .or_else(|| s.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(s)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_defaults() {
        let c = Config::default();
        let s = toml::to_string_pretty(&c).unwrap();
        let d: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, d);
        let empty: Config = toml::from_str("").unwrap();
        assert_eq!(empty, Config::default());
    }

    #[test]
    fn uci_import() {
        let uci = r#"
config linkmeter 'daemon'
	option serial_device 'auto'
	option lidtrack_enabled '1'

config linkmeter 'api'
	option key 'abcdef0123456789abcdef0123456789'
	option allowcors '1'

config linkmeter 'alarms'
	option emailH1 '1'
	option raH1 '2'
	option spL0 '250'

config linkmeter 'alarms_email'
	option toaddress 'me@example.com'

config linkmeter 'alarms_push'
	option provider 'pushover'
	option pushoveruser 'u'
	option pushovertoken 't'

config linkmeter 'ramp'
	option enabled '1'
	option watch '1 2'
	option trigger '180'
	option target '205'
"#;
        let c = import_uci(uci, Config::default());
        assert!(c.daemon.lidtrack_enabled);
        assert_eq!(c.api.key, "abcdef0123456789abcdef0123456789");
        let a = c.action(1, 'H');
        assert!(a.email);
        assert_eq!(a.action, RingAction::Disable);
        assert_eq!(c.action(0, 'L').setpoint.as_deref(), Some("250"));
        assert_eq!(c.alarms.email.to_address, "me@example.com");
        assert_eq!(c.alarms.push.provider, PushProvider::Pushover);
        assert!(c.ramp.enabled);
        assert_eq!(c.ramp.watch, vec![1, 2]);
    }
}
