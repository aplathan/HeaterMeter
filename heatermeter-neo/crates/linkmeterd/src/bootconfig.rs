//! First-boot / headless configuration from a plain text file on the SD
//! card's FAT boot partition (`/boot/firmware/heatermeter.txt`), so a new
//! device can be set up by editing a file on any computer.
//!
//! Format: `key = value` lines, `#` starts a comment line. Values may be
//! quoted. Secrets (passwords) are removed from the file once applied;
//! other values are remembered so they are only applied again when changed.

use crate::app::AppRef;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "heatermeter.txt";
const SECRETS: [&str; 3] = ["wifi_password", "admin_password", "ap_password"];

/// Candidate locations (Bookworm and later mount the FAT partition at /boot/firmware).
pub fn locate() -> Option<PathBuf> {
    ["/boot/firmware", "/boot"].iter().map(|d| Path::new(d).join(FILE_NAME)).find(|p| p.is_file())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub line: usize,
    pub key: String,
    pub value: String,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

pub fn parse(text: &str) -> Vec<Entry> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
                return None;
            }
            let (k, v) = l.split_once('=').or_else(|| l.split_once(':'))?;
            let key = k.trim().to_ascii_lowercase().replace([' ', '-'], "_");
            Some(Entry { line: i, key, value: unquote(v) })
        })
        .collect()
}

/// Blank applied secrets and leave a note above them. Keeps CRLF line endings.
pub fn redact(text: &str, applied: &[String], stamp: &str) -> String {
    let crlf = text.contains("\r\n");
    let bom = text.starts_with('\u{feff}');
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<String> = Vec::new();
    let entries = parse(body);
    for (i, line) in body.lines().enumerate() {
        if let Some(e) = entries.iter().find(|e| e.line == i && applied.contains(&e.key) && !e.value.is_empty()) {
            let note = format!("# {}: asetettu / applied {stamp}", e.key);
            if out.last().is_some_and(|l| l.starts_with(&format!("# {}: asetettu", e.key))) {
                out.pop();
            }
            out.push(note);
            out.push(format!("{} =", e.key));
        } else {
            out.push(line.to_string());
        }
    }
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut s = out.join(nl);
    s.push_str(nl);
    if bom { format!("\u{feff}{s}") } else { s }
}

fn valid_hostname(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && !h.starts_with('-')
        && !h.ends_with('-')
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

async fn sh(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new(cmd).args(args).output().await.map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("{cmd}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file())).unwrap_or(false)
}

/// Apply the boot configuration file if present. Returns log lines.
pub async fn apply(app: &AppRef) -> Vec<String> {
    let Some(path) = locate() else { return Vec::new() };
    apply_file(app, &path).await
}

pub async fn apply_file(app: &AppRef, path: &Path) -> Vec<String> {
    let mut log = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else { return log };
    let vals: BTreeMap<String, String> = parse(&text).into_iter().map(|e| (e.key, e.value)).collect();
    let state_path = app.config().daemon.data_dir.join("bootconfig.json");
    let mut state: BTreeMap<String, String> =
        std::fs::read_to_string(&state_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    // A value is "new" when it is non-empty and differs from what was applied before.
    // Secrets are always new when present (they are blanked after use).
    const KEYS: [&str; 9] = [
        "wifi_country",
        "hostname",
        "timezone",
        "ap_ssid",
        "ap_password",
        "wifi_password",
        "admin_password",
        "units",
        "wifi_ssid",
    ];
    let pending: BTreeMap<&str, String> = KEYS
        .iter()
        .filter_map(|k| {
            let v = vals.get(*k).filter(|v| !v.is_empty())?;
            (SECRETS.contains(k) || state.get(*k) != Some(v)).then(|| (*k, v.clone()))
        })
        .collect();
    let new = |k: &str| pending.get(k).cloned();
    let mut applied: Vec<String> = Vec::new();
    let mut ok = |k: &str, v: &str, log: &mut Vec<String>, msg: String| {
        log.push(msg);
        applied.push(k.to_string());
        if !SECRETS.contains(&k) {
            state.insert(k.to_string(), v.to_string());
        }
    };

    if let Some(cc) = new("wifi_country") {
        let cc = cc.to_ascii_uppercase();
        if cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_alphabetic()) {
            let r = if has("raspi-config") {
                sh("raspi-config", &["nonint", "do_wifi_country", &cc]).await
            } else {
                sh("iw", &["reg", "set", &cc]).await
            };
            let _ = sh("rfkill", &["unblock", "wifi"]).await;
            match r {
                Ok(()) => ok("wifi_country", &cc, &mut log, format!("wifi country set to {cc}")),
                Err(e) => log.push(format!("wifi_country: {e}")),
            }
        } else {
            log.push(format!("wifi_country: invalid country code {cc:?}"));
        }
    }

    if let Some(h) = new("hostname") {
        let h = h.to_ascii_lowercase();
        if valid_hostname(&h) {
            let r = if has("raspi-config") {
                sh("raspi-config", &["nonint", "do_hostname", &h]).await
            } else {
                sh("hostnamectl", &["set-hostname", &h]).await
            };
            match r {
                Ok(()) => {
                    let _ = sh("hostnamectl", &["set-hostname", &h]).await;
                    let _ = sh("systemctl", &["try-restart", "avahi-daemon"]).await;
                    ok("hostname", &h, &mut log, format!("hostname set to {h}"))
                }
                Err(e) => log.push(format!("hostname: {e}")),
            }
        } else {
            log.push(format!("hostname: invalid name {h:?}"));
        }
    }

    if let Some(tz) = new("timezone") {
        if !tz.contains("..") && Path::new("/usr/share/zoneinfo").join(&tz).is_file() {
            match sh("timedatectl", &["set-timezone", &tz]).await {
                Ok(()) => ok("timezone", &tz, &mut log, format!("timezone set to {tz}")),
                Err(e) => log.push(format!("timezone: {e}")),
            }
        } else {
            log.push(format!("timezone: unknown time zone {tz:?}"));
        }
    }

    // Access point settings
    let ap_ssid = new("ap_ssid");
    let ap_pw = new("ap_password");
    if ap_ssid.is_some() || ap_pw.is_some() {
        if let Some(p) = &ap_pw
            && !(8..=63).contains(&p.len())
        {
            log.push("ap_password: must be 8-63 characters, ignored".into());
        }
        let ap_pw = ap_pw.filter(|p| (8..=63).contains(&p.len()));
        let r = app.update_config(|c| {
            if let Some(s) = &ap_ssid {
                c.wifi.ap_ssid = s.chars().take(32).collect();
            }
            if let Some(p) = &ap_pw {
                c.wifi.ap_password = p.clone();
            }
        });
        if r.is_ok() {
            if let Some(s) = &ap_ssid {
                ok("ap_ssid", s, &mut log, format!("access point name set to {s}"));
            }
            if let Some(p) = &ap_pw {
                ok("ap_password", p, &mut log, "access point password set".into());
            }
        }
    }

    // Client wifi: apply when the SSID changed or a password was given
    let ssid = vals.get("wifi_ssid").filter(|v| !v.is_empty()).cloned();
    let pw = new("wifi_password");
    let ssid_changed = new("wifi_ssid").is_some();
    if let Some(ssid) = ssid
        && (pw.is_some() || ssid_changed)
    {
        let pw = pw.unwrap_or_default();
        match crate::wifi::save_network(&app.config().wifi, &ssid, &pw).await {
            Ok(_) => {
                ok("wifi_ssid", &ssid, &mut log, format!("wifi network {ssid} saved"));
                if !pw.is_empty() {
                    ok("wifi_password", &pw, &mut log, "wifi password set".into());
                }
            }
            Err(e) => log.push(format!("wifi: {e}")),
        }
    }

    if let Some(pw) = new("admin_password") {
        if pw.chars().count() >= 6 {
            match crate::http::hash_password(&pw) {
                Ok(hash) => {
                    if app.update_config(|c| c.auth.password_hash = hash).is_ok() {
                        let _ = app.storage.auth_clear();
                        ok("admin_password", &pw, &mut log, "admin password set".into());
                    }
                }
                Err(e) => log.push(format!("admin_password: {e}")),
            }
        } else {
            log.push("admin_password: must be at least 6 characters, ignored".into());
        }
    }

    if let Some(u) = new("units") {
        let u = u.trim().to_ascii_uppercase();
        let u = u.trim_start_matches('°');
        if u == "C" || u == "F" {
            app.set_pending_units(u.chars().next());
            ok("units", u, &mut log, format!("temperature units will be set to {u}"));
        } else {
            log.push(format!("units: must be C or F, got {u:?}"));
        }
    }

    if !applied.is_empty() {
        if let Ok(s) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&state_path, s);
        }
        let secrets: Vec<String> = applied.iter().filter(|k| SECRETS.contains(&k.as_str())).cloned().collect();
        if !secrets.is_empty() {
            let stamp = chrono_like_now();
            if let Err(e) = std::fs::write(path, redact(&text, &secrets, &stamp)) {
                log.push(format!("cannot rewrite {}: {e}", path.display()));
            }
        }
    }
    log
}

/// `YYYY-MM-DD HH:MM` (UTC) without a date crate.
fn chrono_like_now() -> String {
    let secs = crate::app::now();
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}# comment\r\nwifi_ssid = \"Koti WLAN\"\r\nwifi_password = salainen123\r\nWifi-Country: fi\r\nadmin_password =\r\n";

    #[test]
    fn parses() {
        let e = parse(SAMPLE);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].key, "wifi_ssid");
        assert_eq!(e[0].value, "Koti WLAN");
        assert_eq!(e[1].value, "salainen123");
        assert_eq!(e[2].key, "wifi_country");
        assert_eq!(e[2].value, "fi");
        assert_eq!(e[3].value, "");
    }

    #[test]
    fn redacts_secrets() {
        let out = redact(SAMPLE, &["wifi_password".into()], "2026-11-22 20:00 UTC");
        assert!(out.starts_with('\u{feff}'));
        assert!(out.contains("\r\n# wifi_password: asetettu / applied 2026-11-22 20:00 UTC\r\nwifi_password =\r\n"));
        assert!(!out.contains("salainen123"));
        assert!(out.contains("wifi_ssid = \"Koti WLAN\""));
        // redacting again replaces the note instead of stacking them
        let again =
            redact(&out.replace("wifi_password =", "wifi_password = uusi12345"), &["wifi_password".into()], "x");
        assert_eq!(again.matches("# wifi_password: asetettu").count(), 1);
    }

    #[test]
    fn packaged_template_parses() {
        let e = parse(include_str!("../../../packaging/heatermeter.txt"));
        let keys: Vec<&str> = e.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "wifi_ssid",
                "wifi_password",
                "wifi_country",
                "admin_password",
                "hostname",
                "timezone",
                "units",
                "ap_ssid",
                "ap_password"
            ]
        );
        // secrets ship empty
        assert!(e.iter().filter(|e| SECRETS.contains(&e.key.as_str())).all(|e| e.value.is_empty()));
    }

    #[test]
    fn hostnames() {
        assert!(valid_hostname("heatermeter"));
        assert!(valid_hostname("grilli-2"));
        assert!(!valid_hostname("-bad"));
        assert!(!valid_hostname("with space"));
    }

    #[test]
    fn date_format() {
        assert_eq!(chrono_like_now().len(), "2026-11-22 20:00 UTC".len());
    }
}
