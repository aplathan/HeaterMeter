//! Wifi management through NetworkManager's `nmcli` (Raspberry Pi OS
//! Bookworm and later), replacing OpenWrt netifd + `wifi-client`/`wifi-ap`.

use crate::config::WifiConfig;
use serde::Serialize;
use std::time::Duration;

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
    pub ip: Option<String>,
}

const HOTSPOT_CON: &str = "heatermeter-ap";

async fn nmcli(args: &[&str]) -> Result<String, String> {
    let fut = tokio::process::Command::new("nmcli").args(args).output();
    let out = tokio::time::timeout(Duration::from_secs(45), fut)
        .await
        .map_err(|_| "nmcli timed out".to_string())?
        .map_err(|e| format!("nmcli: {e}"))?;
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

pub async fn scan(cfg: &WifiConfig) -> Result<Vec<Network>, String> {
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

pub async fn status(cfg: &WifiConfig) -> WifiStatus {
    let Ok(text) = nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"]).await else {
        return WifiStatus::default();
    };
    for line in text.lines() {
        let f = split_terse(line);
        if f.len() >= 4 && f[0] == cfg.interface {
            return WifiStatus {
                available: true,
                state: f[2].clone(),
                hotspot: f[3] == HOTSPOT_CON,
                connection: f[3].clone(),
                ip: if_addrs::get_if_addrs()
                    .ok()
                    .and_then(|a| a.into_iter().find(|i| i.name == cfg.interface && i.ip().is_ipv4()))
                    .map(|i| i.ip().to_string()),
            };
        }
    }
    WifiStatus { available: false, ..Default::default() }
}

pub async fn connect(cfg: &WifiConfig, ssid: &str, password: &str) -> Result<String, String> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("invalid SSID".into());
    }
    let _ = nmcli(&["connection", "down", HOTSPOT_CON]).await;
    let mut args = vec!["device", "wifi", "connect", ssid, "ifname", cfg.interface.as_str()];
    if !password.is_empty() {
        args.push("password");
        args.push(password);
    }
    nmcli(&args).await.map(|s| s.trim().to_string())
}

pub async fn start_hotspot(cfg: &WifiConfig) -> Result<String, String> {
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
        "autoconnect",
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
    ];
    if !cfg.ap_password.is_empty() {
        args.extend(["wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.psk", cfg.ap_password.as_str()]);
    }
    nmcli(&args).await?;
    nmcli(&["connection", "up", HOTSPOT_CON]).await.map(|s| s.trim().to_string())
}

/// Start the fallback access point if nothing is connected a while after boot.
pub async fn fallback_task(app: crate::app::AppRef) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let cfg = app.config().wifi;
    if !cfg.enabled || !cfg.fallback_ap {
        return;
    }
    if crate::app::primary_ip().is_some() {
        return;
    }
    tracing::warn!("no network connection, starting access point {}", cfg.ap_ssid);
    match start_hotspot(&cfg).await {
        Ok(_) => app.device.toast("Wifi AP", &cfg.ap_ssid),
        Err(e) => tracing::error!("cannot start access point: {e}"),
    }
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
}
