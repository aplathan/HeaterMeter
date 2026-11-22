//! `/api/v2`: JSON API used by the new web UI.

use super::*;
use crate::alarms::{self, Channel};
use crate::config::{AlarmsConfig, ApiConfig, SmtpConfig, UiConfig, WifiConfig};
use crate::firmware;
use crate::wifi;
use axum::Json;
use axum::extract::{Multipart, Path, Query};
use axum::routing::{get, post};
use lm_core::plugins::RampParams;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn routes() -> Router<AppRef> {
    Router::new()
        .route("/info", get(info))
        .route("/status", get(status))
        .route("/config", get(device_config))
        .route("/events", get(events))
        .route("/set", post(set))
        .route("/reboot", post(reboot))
        .route("/toast", post(toast))
        .route("/sessions", get(sessions))
        .route("/sessions/new", post(session_new))
        .route("/sessions/{id}", axum::routing::patch(session_rename).delete(session_delete))
        .route("/sessions/{id}/continue", post(session_continue))
        .route("/sessions/{id}/history", get(history))
        .route("/sessions/{id}/csv", get(csv))
        .route("/settings", get(settings_get).put(settings_put))
        .route("/apikey/regenerate", post(apikey_regen))
        .route("/alarms/test", post(alarm_test))
        .route("/notify/test", post(notify_test))
        .route("/firmware", get(firmware_info).post(firmware_post))
        .route("/wifi/status", get(wifi_status))
        .route("/wifi/scan", get(wifi_scan))
        .route("/wifi/connect", post(wifi_connect))
        .route("/wifi/hotspot", post(wifi_hotspot).delete(wifi_hotspot_stop))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout_h))
        .route("/auth/password", post(password))
        .route("/unkprobe/{op}", post(unkprobe))
        .route("/diag/peaks", get(peaks))
}

type R = Response<Body>;

fn err(status: StatusCode, msg: impl Into<String>) -> R {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn forbidden() -> R {
    err(StatusCode::UNAUTHORIZED, "login required")
}

macro_rules! require_admin {
    ($app:expr, $h:expr) => {
        if !is_admin(&$app, &$h) {
            return forbidden();
        }
    };
}

macro_rules! require_read {
    ($app:expr, $h:expr) => {
        if !can_read(&$app, &$h) {
            return forbidden();
        }
    };
}

// ------------------------------------------------------------------ status

async fn info(State(app): State<AppRef>, h: HeaderMap) -> R {
    let cfg = app.config();
    let up = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .map(|v| v as i64);
    let cpu_temp = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|v| v / 1000.0);
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "ucid": app.device.read(|c| c.ucid()),
        "connected": app.device.connected(),
        "configured": app.device.read(|c| c.config_complete()),
        "port": app.device.port(),
        "hostname": crate::app::hostname(),
        "ip": crate::app::primary_ip(),
        "uptime": up,
        "daemon_uptime": app.started.elapsed().as_secs(),
        "cpu_temp": cpu_temp,
        "admin": is_admin(&app, &h),
        "password_set": password_set(&app),
        "guest_read": cfg.auth.guest_read(),
        "simulator": cfg.daemon.serial_device == "sim",
        "firmware_enabled": cfg.firmware.enabled,
        "wifi_enabled": cfg.wifi.enabled,
        "lidtrack_enabled": cfg.daemon.lidtrack_enabled,
        "ramp": cfg.ramp,
        "session": app.storage.current_session(),
        "time": now(),
    }))
    .into_response()
}

async fn status(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_read!(app, h);
    json_raw(app.device.status_json())
}

async fn device_config(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_read!(app, h);
    json_raw(app.device.config_json())
}

async fn events(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_read!(app, h);
    event_stream(&app, false, false)
}

// ------------------------------------------------------------------ set

/// Shared implementation of `/luci/admin/lm/set`, `/luci/lm/api/config`
/// (POST) and `/api/v2/set` (port of `lmadmin.api_set`).
pub async fn api_set(app: &AppRef, user: &str, vals: Vec<(String, String)>) -> (StatusCode, String) {
    let mut vals = vals;
    // rawset: "/set?a=1&b=2" exploded into individual values
    if let Some(raw) = vals.iter().find(|(k, _)| k == "rawset").map(|(_, v)| v.clone()) {
        let raw = raw.trim_start_matches('/').trim_start_matches("set?").to_string();
        vals = raw
            .split(['&', ';'])
            .filter_map(|p| p.split_once('=').filter(|(k, v)| !k.is_empty() && !v.is_empty()))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
    }
    vals.retain(|(k, _)| k != "apikey");

    let mut out = String::new();
    let mut changed_cfg = false;
    if let Some(key) = take(&mut vals, "lm_apikey").filter(|v| !v.is_empty()) {
        let _ = app.update_config(|c| c.api.key = key);
        out.push_str("API key updated\n");
        changed_cfg = true;
    }
    if let Some(lt) = take(&mut vals, "lm_lidtrack_enabled").filter(|v| !v.is_empty()) {
        let on = lt == "1";
        let _ = app.update_config(|c| c.daemon.lidtrack_enabled = on);
        out.push_str(&format!("LidTrack {}\n", if on { "enabled" } else { "disabled" }));
        changed_cfg = true;
    }
    if vals.is_empty() {
        if !changed_cfg {
            return (StatusCode::BAD_REQUEST, "No values specified".into());
        }
        return (StatusCode::OK, out);
    }
    out.push_str(&format!("User {user} setting {} values...\n", vals.len()));
    for (k, v) in vals {
        let v = if k == "sp" { hm_protocol::normalize_setpoint(&v) } else { v };
        match app.device.set(&k, &v) {
            Ok(()) => out.push_str(&format!("{k} to {v} = OK\n")),
            Err(e) => {
                out.push_str(&format!("{k} to {v} = ERR {e}\n"));
                break;
            }
        }
    }
    out.push_str("Done!");
    (StatusCode::OK, out)
}

fn take(vals: &mut Vec<(String, String)>, key: &str) -> Option<String> {
    let pos = vals.iter().position(|(k, _)| k == key)?;
    Some(vals.remove(pos).1)
}

async fn set(State(app): State<AppRef>, h: HeaderMap, Json(body): Json<serde_json::Map<String, Value>>) -> R {
    let admin = is_admin(&app, &h);
    if !admin && !apikey_ok(&app, &h, body.get("apikey").and_then(Value::as_str)) {
        return forbidden();
    }
    let vals: Vec<(String, String)> = body
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s,
                other => other.to_string(),
            };
            (k, s)
        })
        .collect();
    let (st, msg) = api_set(&app, if admin { "admin" } else { "api" }, vals).await;
    let ok = st == StatusCode::OK && !msg.contains("= ERR");
    (st, Json(json!({ "ok": ok, "log": msg }))).into_response()
}

async fn reboot(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    app.device.reboot();
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct ToastReq {
    line1: String,
    #[serde(default)]
    line2: String,
}

async fn toast(State(app): State<AppRef>, h: HeaderMap, Json(t): Json<ToastReq>) -> R {
    require_admin!(app, h);
    app.device.toast(&t.line1, &t.line2);
    Json(json!({ "ok": true })).into_response()
}

// ------------------------------------------------------------------ sessions

async fn sessions(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_read!(app, h);
    match app.storage.sessions() {
        Ok(s) => Json(s).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn session_id(app: &AppRef, id: &str) -> Option<i64> {
    if id == "current" { app.storage.current_session() } else { id.parse().ok() }
}

async fn session_new(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    match app.storage.new_session() {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct RenameReq {
    name: String,
}

async fn session_rename(
    State(app): State<AppRef>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<RenameReq>,
) -> R {
    require_admin!(app, h);
    let Some(id) = session_id(&app, &id) else { return err(StatusCode::NOT_FOUND, "no such session") };
    let name: String = r.name.trim().chars().take(80).collect();
    match app.storage.rename(id, &name) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such session"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn session_delete(State(app): State<AppRef>, h: HeaderMap, Path(id): Path<String>) -> R {
    require_admin!(app, h);
    let Some(id) = session_id(&app, &id) else { return err(StatusCode::NOT_FOUND, "no such session") };
    match app.storage.delete(id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such session"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn session_continue(State(app): State<AppRef>, h: HeaderMap, Path(id): Path<String>) -> R {
    require_admin!(app, h);
    let Some(id) = session_id(&app, &id) else { return err(StatusCode::NOT_FOUND, "no such session") };
    match app.storage.continue_session(id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "no such session"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct HistQuery {
    from: Option<i64>,
    to: Option<i64>,
    step: Option<i64>,
    /// Target number of points when `step` is not given
    points: Option<i64>,
}

async fn history(State(app): State<AppRef>, h: HeaderMap, Path(id): Path<String>, Query(q): Query<HistQuery>) -> R {
    require_read!(app, h);
    let Some(id) = session_id(&app, &id) else { return Json(json!({ "step": 1, "time": [] })).into_response() };
    let range = match app.storage.range(id) {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "step": 1, "time": [] })).into_response(),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let from = q.from.unwrap_or(range.0).max(range.0);
    let to = q.to.unwrap_or(range.1).min(range.1);
    let step = q.step.unwrap_or_else(|| {
        let pts = q.points.unwrap_or(1500).clamp(50, 10000);
        let raw = ((to - from) / pts).max(1);
        [1, 2, 5, 10, 15, 20, 30, 60, 120, 180, 300, 600].into_iter().find(|s| *s >= raw).unwrap_or(raw)
    });
    let session = id;
    let app2 = app.clone();
    match tokio::task::spawn_blocking(move || app2.storage.history(session, from, to, step, false)).await {
        Ok(Ok(h)) => Json(h).into_response(),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn csv(State(app): State<AppRef>, h: HeaderMap, Path(id): Path<String>) -> R {
    require_read!(app, h);
    let Some(id) = session_id(&app, &id) else { return text(StatusCode::NOT_FOUND, "no such session") };
    let Ok(Some((a, b))) = app.storage.range(id) else { return text(StatusCode::NOT_FOUND, "no data") };
    match app.storage.history(id, a, b, 1, false) {
        Ok(h) => (
            [
                (header::CONTENT_TYPE, "text/csv".to_string()),
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"heatermeter-{id}.csv\"")),
            ],
            h.to_csv(true),
        )
            .into_response(),
        Err(e) => text(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ------------------------------------------------------------------ settings

/// The part of the daemon configuration the web UI edits.
#[derive(serde::Serialize, Deserialize)]
struct Settings {
    lidtrack_enabled: bool,
    guest_read: bool,
    api: ApiConfig,
    ramp: RampParams,
    alarms: AlarmsConfig,
    smtp: SmtpConfig,
    ui: UiConfig,
    wifi: WifiConfig,
}

const SECRET_MASK: &str = "********";

async fn settings_get(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let c = app.config();
    let mut smtp = c.smtp.clone();
    if !smtp.password.is_empty() {
        smtp.password = SECRET_MASK.into();
    }
    Json(Settings {
        lidtrack_enabled: c.daemon.lidtrack_enabled,
        guest_read: c.auth.guest_read(),
        api: c.api,
        ramp: c.ramp,
        alarms: c.alarms,
        smtp,
        ui: c.ui,
        wifi: c.wifi,
    })
    .into_response()
}

async fn settings_put(State(app): State<AppRef>, h: HeaderMap, Json(s): Json<Settings>) -> R {
    require_admin!(app, h);
    let ramp = s.ramp.clone();
    let r = app.update_config(|c| {
        c.daemon.lidtrack_enabled = s.lidtrack_enabled;
        c.auth.guest_read = Some(s.guest_read);
        c.api = s.api;
        // keep an active ramp's start point unless ramping was switched off
        let start = if s.ramp.enabled { c.ramp.start_setpoint } else { None };
        c.ramp = RampParams { start_setpoint: start, ..s.ramp };
        c.alarms.actions = s.alarms.actions;
        c.alarms.email = s.alarms.email;
        c.alarms.sms = s.alarms.sms;
        c.alarms.push = s.alarms.push;
        let pw = if s.smtp.password == SECRET_MASK { c.smtp.password.clone() } else { s.smtp.password.clone() };
        c.smtp = SmtpConfig { password: pw, ..s.smtp };
        c.ui = s.ui;
        c.wifi = s.wifi;
    });
    let mut ramp = ramp;
    ramp.start_setpoint = app.config().ramp.start_setpoint;
    app.device.reload_ramp(ramp);
    match r {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn apikey_regen(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let key = random_hex(16);
    let k2 = key.clone();
    match app.update_config(|c| c.api.key = k2) {
        Ok(()) => Json(json!({ "key": key })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ------------------------------------------------------------------ alarms

#[derive(Deserialize)]
struct AlarmTestReq {
    probe: usize,
    #[serde(default)]
    kind: Option<char>,
    #[serde(default)]
    stop: bool,
}

async fn alarm_test(State(app): State<AppRef>, h: HeaderMap, Json(r): Json<AlarmTestReq>) -> R {
    require_admin!(app, h);
    if r.probe > 3 {
        return err(StatusCode::BAD_REQUEST, "probe must be 0-3");
    }
    let kind = r.kind.map(|k| k.to_ascii_uppercase()).filter(|k| *k == 'L' || *k == 'H');
    if r.stop || kind.is_none() {
        app.device.alarm_test(r.probe, None, None);
    } else {
        app.device.alarm_test(r.probe, kind, None);
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct NotifyTestReq {
    channel: Channel,
}

async fn notify_test(State(app): State<AppRef>, h: HeaderMap, Json(r): Json<NotifyTestReq>) -> R {
    require_admin!(app, h);
    let env = alarms::sample_env(&app);
    let cfg = app.config();
    let ev = json!({ "probe": 0, "type": "H", "test": true });
    match alarms::send(&app, &cfg, r.channel, &env, &ev).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "ok": false, "message": e }))).into_response(),
    }
}

// ------------------------------------------------------------------ firmware

async fn firmware_info(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let cfg = app.config().firmware;
    let files: Vec<String> = firmware::bundled(&cfg).iter().map(|p| p.display().to_string()).collect();
    Json(json!({ "enabled": cfg.enabled, "bundled": files, "default": cfg.bundled_hex, "ucid": app.device.read(|c| c.ucid()) }))
        .into_response()
}

/// Read a multipart firmware upload (`hexfile`), or `url` / `bundled` fields.
pub async fn read_fw_source(app: &AppRef, mut mp: Multipart) -> Result<firmware::Source, String> {
    let mut source = None;
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "hexfile" => {
                let data = field.bytes().await.map_err(|e| e.to_string())?;
                if data.len() > 512 * 1024 || !firmware::looks_like_hex(&data) {
                    return Err("uploaded file is not an Intel HEX firmware image".into());
                }
                let path = std::env::temp_dir().join("lm-upload.hex");
                std::fs::write(&path, &data).map_err(|e| e.to_string())?;
                source = Some(firmware::Source::File(path));
            }
            "url" => {
                let u = field.text().await.unwrap_or_default();
                if !u.trim().is_empty() {
                    source = Some(firmware::Source::Url(u.trim().to_string()));
                }
            }
            "bundled" => {
                let f = field.text().await.unwrap_or_default();
                let allowed = firmware::bundled(&app.config().firmware);
                let p = std::path::PathBuf::from(f.trim());
                if !allowed.contains(&p) {
                    return Err("unknown bundled firmware".into());
                }
                source = Some(firmware::Source::File(p));
            }
            _ => {}
        }
    }
    source.ok_or_else(|| "Missing hexfile, url or bundled parameter".into())
}

pub fn spawn_update(app: &AppRef, src: firmware::Source) -> R {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app.clone();
    tokio::spawn(async move {
        firmware::update(&app, src, tx).await;
    });
    line_stream(rx)
}

async fn firmware_post(State(app): State<AppRef>, h: HeaderMap, mp: Multipart) -> R {
    require_admin!(app, h);
    match read_fw_source(&app, mp).await {
        Ok(src) => spawn_update(&app, src),
        Err(e) => text(StatusCode::BAD_REQUEST, e),
    }
}

// ------------------------------------------------------------------ wifi

async fn wifi_status(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let cfg = app.config().wifi;
    if !cfg.enabled {
        return Json(json!({ "available": false, "disabled": true })).into_response();
    }
    Json(wifi::status(&app).await).into_response()
}

async fn wifi_scan(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let cfg = app.config().wifi;
    if !cfg.enabled {
        return err(StatusCode::SERVICE_UNAVAILABLE, "wifi management disabled");
    }
    match wifi::scan(&app).await {
        Ok(n) => Json(n).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
struct ConnectReq {
    ssid: String,
    #[serde(default)]
    password: String,
}

async fn wifi_connect(State(app): State<AppRef>, h: HeaderMap, Json(r): Json<ConnectReq>) -> R {
    require_admin!(app, h);
    let cfg = app.config().wifi;
    if !cfg.enabled {
        return err(StatusCode::SERVICE_UNAVAILABLE, "wifi management disabled");
    }
    if let Err(e) = wifi::validate(&r.ssid, &r.password) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    if app.wifi.ap_active() {
        // The client is (most likely) on our access point, which goes away when
        // we switch: answer first, then switch in the background.
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match wifi::connect(&app2, &r.ssid, &r.password).await {
                Ok(m) => tracing::info!("{m}"),
                Err(e) => tracing::warn!("{e}"),
            }
        });
        let host = crate::app::hostname();
        return Json(json!({ "ok": true, "switching": true, "url": format!("http://{host}.local/") })).into_response();
    }
    match wifi::connect(&app, &r.ssid, &r.password).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

async fn wifi_hotspot(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    let cfg = app.config().wifi;
    if !cfg.enabled {
        return err(StatusCode::SERVICE_UNAVAILABLE, "wifi management disabled");
    }
    match wifi::start_ap(&app, true).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

async fn wifi_hotspot_stop(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_admin!(app, h);
    match wifi::stop_ap(&app).await {
        Ok(m) => Json(json!({ "ok": true, "message": m })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

// ------------------------------------------------------------------ auth

#[derive(Deserialize)]
struct LoginReq {
    password: String,
}

async fn login(State(app): State<AppRef>, Json(r): Json<LoginReq>) -> R {
    let hash = app.config().auth.password_hash;
    if hash.is_empty() || !verify_password(&r.password, &hash) {
        tokio::time::sleep(Duration::from_millis(800)).await;
        return err(StatusCode::UNAUTHORIZED, "wrong password");
    }
    match new_session_cookie(&app) {
        Some(c) => ([(header::SET_COOKIE, c)], Json(json!({ "ok": true }))).into_response(),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "session store failed"),
    }
}

async fn logout_h(State(app): State<AppRef>, h: HeaderMap) -> R {
    let c = logout(&app, &h);
    ([(header::SET_COOKIE, c)], Json(json!({ "ok": true }))).into_response()
}

#[derive(Deserialize)]
struct PasswordReq {
    #[serde(default)]
    current: String,
    new: String,
}

async fn password(State(app): State<AppRef>, h: HeaderMap, Json(r): Json<PasswordReq>) -> R {
    let existing = app.config().auth.password_hash;
    // Changing an existing password requires being logged in and the current password
    if !existing.is_empty() && (!is_admin(&app, &h) || !verify_password(&r.current, &existing)) {
        tokio::time::sleep(Duration::from_millis(800)).await;
        return err(StatusCode::UNAUTHORIZED, "current password is wrong");
    }
    if r.new.chars().count() < 6 {
        return err(StatusCode::BAD_REQUEST, "password must be at least 6 characters");
    }
    let hash = match hash_password(&r.new) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if let Err(e) = app.update_config(|c| c.auth.password_hash = hash) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let _ = app.storage.auth_clear();
    match new_session_cookie(&app) {
        Some(c) => ([(header::SET_COOKIE, c)], Json(json!({ "ok": true }))).into_response(),
        None => Json(json!({ "ok": true })).into_response(),
    }
}

// ------------------------------------------------------------------ diagnostics

async fn unkprobe(State(app): State<AppRef>, h: HeaderMap, Path(op): Path<String>) -> R {
    require_admin!(app, h);
    match op.as_str() {
        "start" => {
            app.device.with_core(|c| c.with_ctx(|p, ctx| p.unkprobe.start(ctx)));
            Json(json!({ "ok": true })).into_response()
        }
        "stop" => {
            let ok = app.device.with_core(|c| c.with_ctx(|p, ctx| p.unkprobe.stop(ctx)));
            Json(json!({ "ok": ok })).into_response()
        }
        "fit" => match app.device.read(|c| c.unkprobe.fit_json()) {
            Some(j) => json_raw(j),
            None => err(StatusCode::CONFLICT, "calibration not running"),
        },
        "points" => Json(app.device.read(|c| c.unkprobe.points())).into_response(),
        "csv" => match app.device.read(|c| c.unkprobe.csv()) {
            Some(c) => text(StatusCode::OK, c),
            None => err(StatusCode::CONFLICT, "calibration not running"),
        },
        _ => err(StatusCode::NOT_FOUND, "unknown operation"),
    }
}

async fn peaks(State(app): State<AppRef>, h: HeaderMap) -> R {
    require_read!(app, h);
    let (state, text) = app.device.read(|c| (serde_json::to_value(&c.peaks.state).ok(), c.peaks.describe()));
    Json(json!({ "state": state, "text": text })).into_response()
}
