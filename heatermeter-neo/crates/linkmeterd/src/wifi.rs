//! Wifi management through NetworkManager's `nmcli` (Raspberry Pi OS
//! Bookworm and later), replacing OpenWrt netifd + `wifi-client`/`wifi-ap`.
//!
//! The device is headless (HeaterMeter only has a 2-line LCD and a 4-way
//! button), so the wifi access point is the way in: it starts automatically
//! when no network is reachable, can be toggled from the LCD network menu, and
//! switching to a client network rolls back to the AP if joining fails.

use crate::app::AppRef;
use crate::config::WifiConfig;
use serde::Serialize;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Network {
    pub ssid: String,
    pub signal: u8,
    pub security: String,
    pub channel: String,
    pub in_use: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WifiStatus {
    pub available: bool,
    pub state: String,
    pub connection: String,
    pub hotspot: bool,
    pub ap_ssid: String,
    pub ip: Option<String>,
}

pub const HOTSPOT_CON: &str = "heatermeter-ap";
pub const AP_ADDR: &str = "192.168.201.1";
const PROFILE_PREFIX: &str = "hm-wifi-";
/// Leave the access point alone while someone used it within this time
const AP_IDLE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Default)]
struct Inner {
    ap_active: bool,
    /// AP was started by the user (LCD/UI), not as a fallback
    ap_manual: bool,
    ap_since: Option<Instant>,
    last_client: Option<Instant>,
    last_scan: Vec<Network>,
    /// Next time to try leaving a fallback AP for a saved network
    retry_at: Option<Instant>,
    retry_backoff: Duration,
}

/// Shared wifi state.
#[derive(Debug, Default)]
pub struct WifiState {
    inner: Mutex<Inner>,
    /// Serialises nmcli state changes
    op: tokio::sync::Mutex<()>,
}

impl WifiState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn ap_active(&self) -> bool {
        self.lock().ap_active
    }

    #[cfg(test)]
    pub fn set_ap_active(&self, on: bool) {
        self.lock().ap_active = on;
    }

    /// Record activity from a client connected to the access point.
    pub fn touch_client(&self) {
        self.lock().last_client = Some(Instant::now());
    }
}

/// Is `ip` an address handed out on the access point network?
pub fn is_ap_client(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[..3] == [192, 168, 201])
}

async fn nmcli(args: &[&str]) -> Result<String, String> {
    run("nmcli", args, 45).await
}

async fn run(cmd: &str, args: &[&str], timeout: u64) -> Result<String, String> {
    let fut = tokio::process::Command::new(cmd).args(args).kill_on_drop(true).output();
    let out = tokio::time::timeout(Duration::from_secs(timeout), fut)
        .await
        .map_err(|_| format!("{cmd} timed out"))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Split an `nmcli -t` line on unescaped `:`.
pub fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

pub fn parse_scan(text: &str) -> Vec<Network> {
    let mut nets: Vec<Network> = Vec::new();
    for line in text.lines() {
        let f = split_terse(line);
        if f.len() < 5 || f[1].is_empty() {
            continue;
        }
        let n = Network {
            in_use: f[0] == "*",
            ssid: f[1].clone(),
            signal: f[2].parse().unwrap_or(0),
            security: f[3].clone(),
            channel: f[4].clone(),
        };
        match nets.iter_mut().find(|x| x.ssid == n.ssid) {
            Some(x) if x.signal < n.signal => *x = Network { in_use: x.in_use || n.in_use, ..n },
            Some(x) => x.in_use |= n.in_use,
            None => nets.push(n),
        }
    }
    nets.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
    nets
}

async fn scan_live(cfg: &WifiConfig) -> Result<Vec<Network>, String> {
    let text = nmcli(&[
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY,CHAN",
        "device",
        "wifi",
        "list",
        "ifname",
        &cfg.interface,
        "--rescan",
        "yes",
    ])
    .await?;
    Ok(parse_scan(&text))
}

/// Scan for networks. While the access point is running the radio usually
/// cannot scan, so the list seen just before the AP started is returned too.
pub async fn scan(app: &AppRef) -> Result<Vec<Network>, String> {
    let cfg = app.config().wifi;
    let live = scan_live(&cfg).await;
    let mut st = app.wifi.lock();
    match live {
        Ok(nets) if !nets.is_empty() => {
            st.last_scan = nets.clone();
            Ok(nets)
        }
        Ok(_) | Err(_) if !st.last_scan.is_empty() => Ok(st.last_scan.clone()),
        Ok(n) => Ok(n),
        Err(e) => Err(e),
    }
}

pub async fn status(app: &AppRef) -> WifiStatus {
    let cfg = app.config().wifi;
    let ap = app.wifi.ap_active();
    let Ok(text) = nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"]).await else {
        return WifiStatus { ap_ssid: cfg.ap_ssid, ..Default::default() };
    };
    for line in text.lines() {
        let f = split_terse(line);
        if f.len() >= 4 && f[0] == cfg.interface {
            let connection = f[3].strip_prefix(PROFILE_PREFIX).unwrap_or(&f[3]).to_string();
            return WifiStatus {
                available: true,
                state: f[2].clone(),
                hotspot: ap || f[3] == HOTSPOT_CON,
                connection,
                ap_ssid: cfg.ap_ssid.clone(),
                ip: if_addrs::get_if_addrs()
                    .ok()
                    .and_then(|a| a.into_iter().find(|i| i.name == cfg.interface && i.ip().is_ipv4()))
                    .map(|i| i.ip().to_string()),
            };
        }
    }
    WifiStatus { ap_ssid: cfg.ap_ssid, ..Default::default() }
}

/// Short status line for the LCD.
pub async fn lcd_line(app: &AppRef) -> Option<String> {
    let s = status(app).await;
    if !s.available {
        return None;
    }
    if s.hotspot {
        return Some(format!("AP {}", s.ap_ssid));
    }
    if s.connection.is_empty() || s.connection == "--" { None } else { Some(s.connection) }
}

/// Validate an SSID / WPA passphrase pair.
pub fn validate(ssid: &str, password: &str) -> Result<(), String> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("SSID must be 1-32 bytes".into());
    }
    if !password.is_empty() && !(8..=63).contains(&password.len()) {
        return Err("wifi password must be 8-63 characters".into());
    }
    Ok(())
}

/// Store a client network profile (does not need the network to be in range).
pub async fn save_network(cfg: &WifiConfig, ssid: &str, password: &str) -> Result<String, String> {
    validate(ssid, password)?;
    let name = format!("{PROFILE_PREFIX}{ssid}");
    let _ = nmcli(&["connection", "delete", &name]).await;
    let mut args = vec![
        "connection",
        "add",
        "type",
        "wifi",
        "ifname",
        cfg.interface.as_str(),
        "con-name",
        name.as_str(),
        "ssid",
        ssid,
        "connection.autoconnect",
        "yes",
        "connection.autoconnect-priority",
        "10",
    ];
    if !password.is_empty() {
        args.extend(["wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.psk", password]);
    }
    nmcli(&args).await?;
    Ok(name)
}

/// Saved client wifi profiles (excluding the access point).
async fn has_saved_networks() -> bool {
    let Ok(text) = nmcli(&["-t", "-f", "NAME,TYPE", "connection", "show"]).await else { return false };
    text.lines().map(split_terse).any(|f| f.len() >= 2 && f[1] == "802-11-wireless" && f[0] != HOTSPOT_CON)
}

/// Any IPv4 connectivity other than the access point itself.
fn online() -> bool {
    if_addrs::get_if_addrs()
        .map(|a| {
            a.iter().any(|i| {
                !i.is_loopback()
                    && matches!(i.ip(), IpAddr::V4(v4) if v4.octets()[..3] != [192, 168, 201] && !v4.is_link_local())
            })
        })
        .unwrap_or(false)
}

/// Number of stations associated with the access point (needs `iw`).
async fn ap_station_count(cfg: &WifiConfig) -> usize {
    run("iw", &["dev", &cfg.interface, "station", "dump"], 10)
        .await
        .map(|t| t.lines().filter(|l| l.starts_with("Station ")).count())
        .unwrap_or(0)
}

fn set_lcd_state(app: &AppRef, ap: bool, cfg: &WifiConfig) {
    app.device.with_core(|c| {
        c.netinfo.ap_active = ap;
        if ap {
            c.netinfo.wifi = Some(format!("AP {}", cfg.ap_ssid));
        }
        ((), Vec::new())
    });
}

async fn ap_up(cfg: &WifiConfig) -> Result<(), String> {
    let _ = nmcli(&["connection", "delete", HOTSPOT_CON]).await;
    let mut args = vec![
        "connection",
        "add",
        "type",
        "wifi",
        "ifname",
        cfg.interface.as_str(),
        "con-name",
        HOTSPOT_CON,
        "connection.autoconnect",
        "no",
        "ssid",
        cfg.ap_ssid.as_str(),
        "802-11-wireless.mode",
        "ap",
        "802-11-wireless.band",
        "bg",
        "ipv4.method",
        "shared",
        "ipv4.addresses",
        "192.168.201.1/24",
        "ipv6.method",
        "disabled",
    ];
    if !cfg.ap_password.is_empty() {
        args.extend(["wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.psk", cfg.ap_password.as_str()]);
    }
    nmcli(&args).await?;
    nmcli(&["connection", "up", HOTSPOT_CON]).await.map(|_| ())
}

/// Start the access point. `manual` = requested by the user.
pub async fn start_ap(app: &AppRef, manual: bool) -> Result<String, String> {
    let cfg = app.config().wifi;
    let _op = app.wifi.op.lock().await;
    // Remember what was visible: scanning is not possible once the AP runs
    if let Ok(nets) = scan_live(&cfg).await
        && !nets.is_empty()
    {
        app.wifi.lock().last_scan = nets;
    }
    ap_up(&cfg).await?;
    {
        let mut st = app.wifi.lock();
        st.ap_active = true;
        st.ap_manual = manual;
        st.ap_since = Some(Instant::now());
        st.last_client = None;
        if st.retry_backoff.is_zero() {
            st.retry_backoff = AP_IDLE;
        }
        st.retry_at = Some(Instant::now() + st.retry_backoff);
    }
    set_lcd_state(app, true, &cfg);
    app.device.toast(&format!("AP {}", cfg.ap_ssid), AP_ADDR);
    tracing::warn!("wifi access point '{}' started ({AP_ADDR})", cfg.ap_ssid);
    Ok(format!("access point {} started at http://{AP_ADDR}/", cfg.ap_ssid))
}

/// Stop the access point; NetworkManager then autoconnects saved networks.
pub async fn stop_ap(app: &AppRef) -> Result<String, String> {
    let cfg = app.config().wifi;
    let _op = app.wifi.op.lock().await;
    let r = nmcli(&["connection", "down", HOTSPOT_CON]).await;
    {
        let mut st = app.wifi.lock();
        st.ap_active = false;
        st.ap_manual = false;
        st.ap_since = None;
    }
    set_lcd_state(app, false, &cfg);
    tracing::info!("wifi access point stopped");
    r.map(|_| "access point stopped".into())
}

/// Join a network from the web UI. If the network cannot be joined the access
/// point is restarted so the device stays reachable.
pub async fn connect(app: &AppRef, ssid: &str, password: &str) -> Result<String, String> {
    let cfg = app.config().wifi;
    let name = save_network(&cfg, ssid, password).await?;
    let was_ap = app.wifi.ap_active();
    if was_ap {
        let _ = stop_ap(app).await;
        // give the radio a moment to switch out of AP mode
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let up = {
        let _op = app.wifi.op.lock().await;
        nmcli(&["connection", "up", &name]).await
    };
    match up {
        Ok(_) => {
            let mut st = app.wifi.lock();
            st.retry_backoff = Duration::ZERO;
            drop(st);
            app.device.with_core(|c| {
                c.netinfo.wifi = Some(ssid.to_string());
                ((), Vec::new())
            });
            Ok(format!("connected to {ssid}"))
        }
        Err(e) => {
            if was_ap || cfg.fallback_ap {
                let _ = start_ap(app, false).await;
            }
            Err(format!("cannot join {ssid}: {e}"))
        }
    }
}

/// Background task: start the access point when offline and periodically try
/// to get back to a saved network from a fallback AP.
pub async fn manager_task(app: AppRef) {
    // Let NetworkManager bring up known networks first
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut offline_checks = 0u32;
    loop {
        let cfg = app.config().wifi;
        if cfg.enabled
            && let Err(e) = manage_once(&app, &cfg, &mut offline_checks).await
        {
            tracing::warn!("wifi: {e}");
        }
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

async fn manage_once(app: &AppRef, cfg: &WifiConfig, offline_checks: &mut u32) -> Result<(), String> {
    if !app.wifi.ap_active() {
        if online() {
            *offline_checks = 0;
            return Ok(());
        }
        *offline_checks += 1;
        if *offline_checks >= 2 && cfg.fallback_ap {
            *offline_checks = 0;
            tracing::warn!("no network connection, starting access point");
            start_ap(app, false).await?;
        }
        return Ok(());
    }

    // AP is running
    if ap_station_count(cfg).await > 0 {
        app.wifi.touch_client();
    }
    let (manual, retry_due, idle) = {
        let st = app.wifi.lock();
        let idle = st.last_client.is_none_or(|t| t.elapsed() > AP_IDLE);
        (st.ap_manual, st.retry_at.is_some_and(|t| Instant::now() >= t), idle)
    };
    if manual || !retry_due || !idle || !has_saved_networks().await {
        return Ok(());
    }
    tracing::info!("trying saved wifi networks");
    stop_ap(app).await?;
    for _ in 0..9 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if online() {
            app.wifi.lock().retry_backoff = Duration::ZERO;
            tracing::info!("connected to a saved wifi network");
            return Ok(());
        }
    }
    {
        let mut st = app.wifi.lock();
        st.retry_backoff = (st.retry_backoff * 2).clamp(AP_IDLE, Duration::from_secs(30 * 60));
    }
    tracing::info!("saved networks not reachable, back to access point");
    start_ap(app, false).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse() {
        assert_eq!(split_terse("a\\:b:c"), vec!["a:b", "c"]);
        let nets = parse_scan(" :Home:70:WPA2:6\n*:Home:80:WPA2:36\n :Open\\:Net:40::11\n :::0:\n");
        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].ssid, "Home");
        assert_eq!(nets[0].signal, 80);
        assert!(nets[0].in_use);
        assert_eq!(nets[1].ssid, "Open:Net");
    }

    #[test]
    fn validation() {
        assert!(validate("Home", "").is_ok());
        assert!(validate("Home", "short").is_err());
        assert!(validate("", "password123").is_err());
        assert!(validate(&"x".repeat(33), "").is_err());
        assert!(is_ap_client("192.168.201.23".parse().unwrap()));
        assert!(!is_ap_client("192.168.1.23".parse().unwrap()));
    }
}
