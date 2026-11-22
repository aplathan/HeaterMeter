//! Legacy endpoints compatible with the OpenWrt/LuCI LinkMeter, used by
//! MeterMonitor, SMUSHI, HeaterMeterClient and third party apps.

use super::*;
use axum::extract::{Multipart, Path, Query, RawQuery};
use axum::response::Redirect;
use axum::routing::{any, get, post};
use std::collections::HashMap;

const API_VERSION: u32 = 1;

pub fn routes() -> Router<AppRef> {
    Router::new()
        .route("/luci", get(to_root))
        .route("/luci/", get(to_root))
        .route("/luci/lm", get(to_root))
        .route("/luci/lm/", get(to_root))
        .route("/luci/lm/hmstatus", get(hmstatus))
        .route("/luci/lm/rfstatus", get(rfstatus))
        .route("/luci/lm/conf", get(conf))
        .route("/luci/lm/hist", get(hist))
        .route("/luci/lm/stream", get(stream))
        .route("/luci/lm/dluri", post(dluri))
        .route("/luci/lm/light", get(light))
        .route("/luci/admin/lm/light", get(light))
        .route("/luci/lm/api", get(api_version))
        .route("/luci/lm/api/version", get(api_version))
        .route("/luci/lm/api/status", get(api_status_root))
        .route("/luci/lm/api/status/{*path}", get(api_status))
        .route("/luci/lm/api/config", any(api_config_root))
        .route("/luci/lm/api/config/{*path}", any(api_config))
        .route("/luci/lm/api/fw", post(api_fw))
        .route("/luci/admin/lm/set", any(admin_set))
        .route("/luci/admin/lm/reboot", get(admin_reboot))
        .route("/luci/admin/lm", get(to_settings))
        .route("/luci/admin/lm/{*rest}", get(to_settings))
        .route("/luci/ssdp", get(ssdp_xml))
}

type R = Response<Body>;

async fn to_root() -> Redirect {
    Redirect::temporary("/")
}

async fn to_settings() -> Redirect {
    Redirect::temporary("/settings")
}

fn cors(app: &AppRef) -> bool {
    app.config().api.allowcors
}

async fn hmstatus(State(app): State<AppRef>) -> R {
    with_cors(json_raw(app.device.status_json()), true)
}

async fn rfstatus(State(app): State<AppRef>) -> R {
    with_cors(json_raw(app.device.read(|c| c.rf_json())), true)
}

async fn conf(State(app): State<AppRef>) -> R {
    with_cors(json_raw(app.device.config_json()), true)
}

async fn hist(State(app): State<AppRef>, Query(q): Query<HashMap<String, String>>) -> R {
    let nancnt = q.get("nancnt").and_then(|v| v.parse::<i64>().ok());
    // `rrd` used to be a file path; it is now a session id (anything else = current)
    let session = q.get("rrd").and_then(|v| v.parse::<i64>().ok());
    let app2 = app.clone();
    let h = match tokio::task::spawn_blocking(move || app2.storage.legacy_history(session, nancnt)).await {
        Ok(Ok(Some(h))) => h,
        _ => return text(StatusCode::SERVICE_UNAVAILABLE, "No database"),
    };
    let body = h.to_csv(q.get("hdr").map(String::as_str) == Some("1"));
    let mut r = if q.get("dl").map(String::as_str) == Some("1") {
        (
            [
                (header::CONTENT_TYPE, "text/csv".to_string()),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"hm.csv\"".to_string()),
            ],
            body,
        )
            .into_response()
    } else {
        text(StatusCode::OK, body)
    };
    if let Ok(v) = HeaderValue::from_str(&format!("max-age={}", h.step)) {
        r.headers_mut().insert(header::CACHE_CONTROL, v);
    }
    with_cors(r, cors(&app))
}

async fn stream(State(app): State<AppRef>) -> R {
    let c = cors(&app);
    event_stream(&app, false, c)
}

/// Echo a data URI back as a file download (graph PNG export).
async fn dluri(mut mp: Multipart) -> R {
    let mut uri = String::new();
    let mut filename = None;
    while let Ok(Some(f)) = mp.next_field().await {
        match f.name() {
            Some("uri") => uri = f.text().await.unwrap_or_default(),
            Some("filename") => filename = f.text().await.ok(),
            _ => {}
        }
    }
    let Some(rest) = uri.strip_prefix("data:") else {
        return text(StatusCode::BAD_REQUEST, "URI must begin with 'data:'");
    };
    let Some((meta, data)) = rest.split_once(',') else { return text(StatusCode::BAD_REQUEST, "URI missing metadata") };
    let mut parts = meta.split(';');
    let mime = parts.next().filter(|m| !m.is_empty()).unwrap_or("text/plain").to_string();
    let b64 = parts.any(|p| p == "base64");
    let bytes = if b64 {
        use base64::Engine;
        match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(b) => b,
            Err(_) => return text(StatusCode::BAD_REQUEST, "bad base64"),
        }
    } else {
        data.as_bytes().to_vec()
    };
    let ext = mime_guess::get_mime_extensions_str(&mime).and_then(|e| e.first()).copied().unwrap_or("txt");
    let name =
        filename.filter(|n| !n.contains(['/', '\\', '"'])).unwrap_or_else(|| format!("heatermeter-{}.{ext}", now()));
    ([(header::CONTENT_TYPE, mime), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], bytes)
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Lo-fi page for tiny screens (light.htm).
async fn light(State(app): State<AppRef>, h: HeaderMap) -> R {
    let admin = password_set(&app) && is_admin(&app, &h);
    let st: serde_json::Value = serde_json::from_str(&app.device.status_json()).unwrap_or_default();
    let mut rows = String::new();
    if let Some(temps) = st["temps"].as_array() {
        for t in temps {
            let c = t["c"].as_f64().map(|v| format!("{v:.1}")).unwrap_or_else(|| "-".into());
            rows.push_str(&format!(
                "<tr><td>{}</td><td class=t>{}</td></tr>",
                html_escape(t["n"].as_str().unwrap_or("")),
                c
            ));
        }
    }
    let set = match &st["set"] {
        serde_json::Value::Number(n) if n.as_f64().unwrap_or(0.0) < 0.0 => format!("{}%", -n.as_f64().unwrap_or(0.0)),
        serde_json::Value::Number(n) => n.to_string(),
        _ => "Off".into(),
    };
    let form = if admin {
        "<form method=post action=\"/luci/admin/lm/set\"><input name=sp size=5 placeholder=\"225\"><button>Set</button></form>"
    } else {
        ""
    };
    let page = format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
<meta http-equiv=refresh content={refresh}><title>HeaterMeter</title>\
<style>body{{font-family:sans-serif;background:#111;color:#eee;margin:8px}}td{{padding:4px 8px}}.t{{font-size:1.6em;text-align:right}}</style>\
</head><body><h3>Setpoint {set} &middot; Fan {fan}%{lid}</h3><table>{rows}</table>{form}<p><a href=\"/\" style=color:#f80>Full view</a></p></body></html>",
        refresh = if admin { 30 } else { 10 },
        fan = st["fan"]["c"].as_f64().unwrap_or(0.0),
        lid = if st["lid"].as_f64().unwrap_or(0.0) > 0.0 { " &middot; Lid open" } else { "" },
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

// ------------------------------------------------------------------ /luci/lm/api

/// API auth (port of api.lua `auth()`); returns Err(response) on failure.
#[allow(clippy::result_large_err)]
fn api_auth(app: &AppRef, h: &HeaderMap, write: bool, form_key: Option<&str>) -> Result<bool, R> {
    let cfg = app.config();
    let cors = cfg.api.allowcors;
    if cfg.api.disabled {
        return Err(with_cors(text(StatusCode::FORBIDDEN, "API disabled"), cors));
    }
    if write && !apikey_ok(app, h, form_key) {
        return Err(with_cors(text(StatusCode::FORBIDDEN, "Invalid or missing apikey"), cors));
    }
    Ok(cors)
}

async fn api_version(State(app): State<AppRef>, h: HeaderMap) -> R {
    let cors = match api_auth(&app, &h, false, None) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let ucid = app.device.read(|c| c.ucid());
    let body = serde_json::json!({ "api": API_VERSION, "ucid": ucid });
    with_cors(json_raw(body.to_string()), cors)
}

async fn api_status_root(State(app): State<AppRef>, h: HeaderMap) -> R {
    api_status_inner(app, h, "").await
}

async fn api_status(State(app): State<AppRef>, h: HeaderMap, Path(path): Path<String>) -> R {
    api_status_inner(app, h, &path).await
}

async fn api_status_inner(app: AppRef, h: HeaderMap, path: &str) -> R {
    let cors = match api_auth(&app, &h, false, None) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let raw = app.device.status_json();
    if path.trim_matches('/').is_empty() {
        return with_cors(json_raw(raw), cors);
    }
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    for k in path.split('/').filter(|s| !s.is_empty()) {
        v = match k.parse::<usize>() {
            Ok(i) if v.is_array() => v.get(i).cloned().unwrap_or_default(),
            _ => v.get(k).cloned().unwrap_or_default(),
        };
    }
    let r = match v {
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => json_raw(v.to_string()),
        serde_json::Value::String(s) => text(StatusCode::OK, s),
        other => text(StatusCode::OK, other.to_string()),
    };
    with_cors(r, cors)
}

async fn read_form(method: &axum::http::Method, query: Option<String>, body: Bytes) -> Vec<(String, String)> {
    let mut v = query.map(|q| parse_form(q.as_bytes())).unwrap_or_default();
    if method == axum::http::Method::POST {
        v.extend(parse_form(&body));
    }
    v
}

async fn api_config_root(
    State(app): State<AppRef>,
    method: axum::http::Method,
    h: HeaderMap,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> R {
    api_config_inner(app, method, h, None, q, body).await
}

async fn api_config(
    State(app): State<AppRef>,
    method: axum::http::Method,
    h: HeaderMap,
    Path(path): Path<String>,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> R {
    let param = path.split('/').find(|s| !s.is_empty()).map(str::to_string);
    api_config_inner(app, method, h, param, q, body).await
}

async fn api_config_inner(
    app: AppRef,
    method: axum::http::Method,
    h: HeaderMap,
    param: Option<String>,
    q: Option<String>,
    body: Bytes,
) -> R {
    let write = method == axum::http::Method::POST;
    let form = read_form(&method, q, body).await;
    let key = form.iter().find(|(k, _)| k == "apikey").map(|(_, v)| v.clone());
    let cors = match api_auth(&app, &h, write, key.as_deref()) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if write {
        let vals = match &param {
            Some(p) => {
                let v = form.iter().find(|(k, _)| k == p || k == "value").map(|(_, v)| v.clone()).unwrap_or_default();
                vec![(p.clone(), v)]
            }
            None => form,
        };
        let (st, msg) = super::api::api_set(&app, "api_write", vals).await;
        return with_cors(text(st, msg), cors);
    }
    let r = match param {
        Some(p) => {
            let v = app.device.read(|c| c.config_map().get(&p).map(|v| v.as_str()));
            text(StatusCode::OK, v.unwrap_or_else(|| "null".into()))
        }
        None => json_raw(app.device.config_json()),
    };
    with_cors(r, cors)
}

async fn api_fw(State(app): State<AppRef>, h: HeaderMap, RawQuery(q): RawQuery, mp: Multipart) -> R {
    // apikey may be in the query string or the multipart body; check the query first
    let qkey = q.as_deref().map(|q| parse_form(q.as_bytes())).unwrap_or_default();
    let qkey = qkey.iter().find(|(k, _)| k == "apikey").map(|(_, v)| v.clone());
    let (src, form_key) = read_fw_multipart(&app, mp).await;
    let key = qkey.or(form_key);
    if let Err(r) = api_auth(&app, &h, true, key.as_deref()) {
        return r;
    }
    match src {
        Ok(s) => super::api::spawn_update(&app, s),
        Err(e) => text(StatusCode::BAD_REQUEST, e),
    }
}

async fn read_fw_multipart(
    app: &AppRef,
    mut mp: Multipart,
) -> (Result<crate::firmware::Source, String>, Option<String>) {
    let mut key = None;
    let mut src = Err("Missing hexfile POST parameter".to_string());
    while let Ok(Some(f)) = mp.next_field().await {
        match f.name() {
            Some("apikey") => key = f.text().await.ok(),
            Some("hexfile") => {
                let data = f.bytes().await.unwrap_or_default();
                src = if crate::firmware::looks_like_hex(&data) && data.len() <= 512 * 1024 {
                    let p = std::env::temp_dir().join("lm-upload.hex");
                    std::fs::write(&p, &data).map(|_| crate::firmware::Source::File(p)).map_err(|e| e.to_string())
                } else {
                    Err("hexfile is not an Intel HEX firmware image".into())
                };
            }
            _ => {}
        }
    }
    let _ = app;
    (src, key)
}

/// `/luci/admin/lm/set`: requires an admin session or the API key.
async fn admin_set(
    State(app): State<AppRef>,
    method: axum::http::Method,
    h: HeaderMap,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> R {
    let form = read_form(&method, q, body).await;
    let key = form.iter().find(|(k, _)| k == "apikey").map(|(_, v)| v.clone());
    let admin = is_admin(&app, &h);
    if !admin && !apikey_ok(&app, &h, key.as_deref()) {
        return text(StatusCode::FORBIDDEN, "Login required");
    }
    let (st, msg) = super::api::api_set(&app, if admin { "root" } else { "api_write" }, form).await;
    text(st, msg)
}

async fn admin_reboot(State(app): State<AppRef>, h: HeaderMap) -> R {
    if !is_admin(&app, &h) {
        return text(StatusCode::FORBIDDEN, "Login required");
    }
    app.device.reboot();
    text(StatusCode::OK, "Rebooting AVR... OK")
}

async fn ssdp_xml(State(app): State<AppRef>, h: HeaderMap) -> R {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("localhost");
    let base = format!("http://{host}/");
    ([(header::CONTENT_TYPE, "text/xml")], crate::discovery::device_xml(&app, &base)).into_response()
}
