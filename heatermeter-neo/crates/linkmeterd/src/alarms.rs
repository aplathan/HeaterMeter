//! System event handling: alarm actions and notifications, ramp
//! persistence and factory reset.

use crate::app::{AppRef, now};
use crate::config::{Config, RingAction};
use crate::device::SysEvent;
use crate::notify;
use lm_core::{AlarmEvent, alarm_set_value};
use std::collections::BTreeMap;
use std::path::Path;
use tokio::sync::mpsc;

pub async fn run(app: AppRef, mut rx: mpsc::UnboundedReceiver<SysEvent>) {
    while let Some(ev) = rx.recv().await {
        match ev {
            SysEvent::Alarm(a) => {
                let app = app.clone();
                tokio::spawn(async move {
                    let log = handle_alarm(&app, &a).await;
                    for l in log {
                        tracing::info!(target: "lmalarm", "{l}");
                    }
                });
            }
            SysEvent::RampStarted(sp) => {
                let _ = app.update_config(|c| c.ramp.start_setpoint = Some(sp));
            }
            SysEvent::RampCancelled => {
                let _ = app.update_config(|c| {
                    c.ramp.enabled = false;
                    c.ramp.start_setpoint = None;
                });
            }
            SysEvent::ResetConfig => {
                tracing::warn!("HeaterMeter requested configuration reset");
                let r = app.update_config(|c| {
                    let keep_daemon = c.daemon.clone();
                    *c = Config::default();
                    c.daemon = keep_daemon;
                    c.daemon.lidtrack_enabled = false;
                    c.api.key = crate::app::random_hex(16);
                });
                let _ = app.storage.auth_clear();
                app.device.reload_ramp(app.config().ramp);
                if let Err(e) = r {
                    tracing::error!("cannot save config: {e}");
                }
            }
            SysEvent::ConfigComplete => {
                tracing::info!("HeaterMeter configuration received");
                if let Some(u) = app.take_pending_units() {
                    tokio::spawn(apply_units(app.clone(), u));
                }
            }
            SysEvent::WifiAp(start) => {
                let app = app.clone();
                tokio::spawn(async move {
                    let r =
                        if start { crate::wifi::start_ap(&app, true).await } else { crate::wifi::stop_ap(&app).await };
                    if let Err(e) = r {
                        tracing::error!("wifi access point: {e}");
                        app.device.toast("Wifi AP failed", &e.chars().take(16).collect::<String>());
                    }
                });
            }
        }
    }
}

/// Switch HeaterMeter's temperature units (from the boot config file),
/// converting the current setpoint like the web UI does.
async fn apply_units(app: AppRef, u: char) {
    // wait for the first status update to know the setpoint
    for _ in 0..20 {
        if app.device.read(|c| c.status.has_update()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let (cur, sp) = app
        .device
        .read(|c| (c.config().and_then(|m| m.get("u").map(|v| v.as_str())), c.status.set.parse::<f64>().ok()));
    if cur.as_deref() == Some(&u.to_string()) {
        return;
    }
    let value = match sp {
        Some(sp) if sp > 0.0 => {
            let conv = if u == 'C' { (sp - 32.0) * 5.0 / 9.0 } else { sp * 9.0 / 5.0 + 32.0 };
            format!("{}{u}", conv.round() as i64)
        }
        _ => u.to_string(),
    };
    tracing::info!("setting temperature units to {u} (sp={value})");
    let _ = app.device.set("sp", &value);
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Run the user's alarm scripts. Returns false if a script asked to stop
/// further processing (non-zero exit).
async fn run_scripts(dir: &Path, a: &AlarmEvent, log: &mut Vec<String>) -> bool {
    for name in ["alarm-all".to_string(), format!("alarm-{}{}", a.probe, a.kind)] {
        let path = dir.join(&name);
        if !is_executable(&path) {
            continue;
        }
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg(&path).current_dir(dir).env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin");
        for (k, v) in &a.env {
            if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                cmd.env(k, v);
            }
        }
        let fut = cmd.output();
        match tokio::time::timeout(std::time::Duration::from_secs(60), fut).await {
            Ok(Ok(out)) => {
                let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
                for l in text.lines() {
                    log.push(format!("{name}: {l}"));
                }
                if !out.status.success() {
                    log.push(format!("{name} exited with {}, stopping alarm processing", out.status));
                    return false;
                }
            }
            Ok(Err(e)) => log.push(format!("{name}: {e}")),
            Err(_) => log.push(format!("{name}: timed out")),
        }
    }
    true
}

/// Execute all configured actions for a ringing alarm.
pub async fn handle_alarm(app: &AppRef, a: &AlarmEvent) -> Vec<String> {
    let cfg = app.config();
    let mut log = Vec::new();
    let mut env = a.env.clone();
    env.entry("ip".into()).or_insert_with(|| crate::app::primary_ip().unwrap_or_default());
    env.insert("hostname".into(), crate::app::hostname());
    let a = AlarmEvent { env: env.clone(), ..a.clone() };

    if !run_scripts(&cfg.alarms.scripts_dir, &a, &mut log).await {
        return log;
    }

    let action = cfg.action(a.probe, a.kind);
    if !a.test {
        match action.action {
            RingAction::Ring => {}
            RingAction::Silence => {
                log.push("Auto-silencing alarm".into());
                let _ = app.device.set("al", &alarm_set_value(a.probe, a.kind, "0"));
            }
            RingAction::Disable => {
                log.push("Auto-disabling alarm".into());
                let t = a.threshold.trim_start_matches('-');
                let _ = app.device.set("al", &alarm_set_value(a.probe, a.kind, &format!("-{t}")));
            }
        }
        if let Some(sp) = action.setpoint.as_deref().filter(|s| !s.is_empty()) {
            log.push(format!("Changing setpoint to {sp}"));
            let _ = app.device.set("sp", &hm_protocol::normalize_setpoint(sp));
        }
    }

    let event = serde_json::json!({
        "probe": a.probe, "type": a.kind.to_string(), "threshold": a.threshold,
        "name": env.get("pn"), "temp": env.get("pcurr"), "time": now(), "test": a.test,
    });
    let mut tasks = Vec::new();
    if action.email && !cfg.alarms.email.to_address.is_empty() {
        tasks.push(("email", Channel::Email));
    }
    if action.sms && !cfg.alarms.sms.to_address.is_empty() {
        tasks.push(("sms", Channel::Sms));
    }
    if action.push {
        tasks.push(("push", Channel::Push));
    }
    for (name, ch) in tasks {
        match send(app, &cfg, ch, &env, &event).await {
            Ok(m) => log.push(m),
            Err(e) => log.push(format!("{name}: {e}")),
        }
    }
    log
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Email,
    Sms,
    Push,
}

pub async fn send(
    app: &AppRef,
    cfg: &Config,
    ch: Channel,
    env: &BTreeMap<String, String>,
    event: &serde_json::Value,
) -> notify::NResult {
    match ch {
        Channel::Email => {
            let e = &cfg.alarms.email;
            if e.to_address.is_empty() {
                return Err("no email address configured".into());
            }
            notify::send_email(
                &cfg.smtp,
                &notify::expand(&e.from_name, env),
                &e.to_address,
                &e.to_name,
                &notify::expand(&e.subject, env),
                &notify::expand(&e.body, env),
            )
            .await
        }
        Channel::Sms => {
            let s = &cfg.alarms.sms;
            if s.to_address.is_empty() {
                return Err("no SMS gateway address configured".into());
            }
            let msg = notify::expand(&s.message, env);
            notify::send_email(&cfg.smtp, "HeaterMeter", &s.to_address, "", "HeaterMeter", &msg).await
        }
        Channel::Push => notify::send_push(&app.http, &cfg.alarms.push, env, event).await,
    }
}

/// Example variables for notification tests.
pub fn sample_env(app: &AppRef) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> =
        app.device.read(|c| c.config_map()).into_iter().map(|(k, v)| (k, v.as_str())).collect();
    env.insert("al_probe".into(), "0".into());
    env.insert("al_type".into(), "H".into());
    env.insert("al_thresh".into(), "250".into());
    env.insert("al_prep".into(), "above".into());
    env.insert("pn".into(), env.get("pn0").cloned().unwrap_or_else(|| "Pit".into()));
    env.insert("pcurr".into(), env.get("pcurr0").cloned().unwrap_or_else(|| "251".into()));
    env.insert("ip".into(), crate::app::primary_ip().unwrap_or_default());
    env.insert("hostname".into(), crate::app::hostname());
    env
}
