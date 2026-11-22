//! HTTP server: modern JSON API (`/api/v2`), the legacy LuCI-compatible
//! endpoints (`/luci/...`) and the embedded single page app.

pub mod api;
pub mod legacy;

use crate::app::{AppRef, now, random_hex};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode, Uri, header};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::StreamExt;
use std::convert::Infallible;
use std::time::Duration;

#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
#[allow_missing = true]
struct Assets;

pub const SESSION_COOKIE: &str = "lmsession";
pub const SESSION_TTL: i64 = 86400;

pub fn router(app: AppRef) -> Router {
    Router::new()
        .merge(legacy::routes())
        .nest("/api/v2", api::routes())
        .route("/user.css", get(user_css))
        .fallback(get(static_files))
        .with_state(app)
}

// ------------------------------------------------------------------ helpers

pub fn text(status: StatusCode, body: impl Into<String>) -> Response<Body> {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body.into()).into_response()
}

pub fn json_raw(body: String) -> Response<Body> {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

pub fn with_cors(mut r: Response<Body>, cors: bool) -> Response<Body> {
    if cors {
        r.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    }
    r
}

/// Parse `application/x-www-form-urlencoded` data preserving order.
pub fn parse_form(data: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(data).into_owned().collect()
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|c| {
        c.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{SESSION_COOKIE}=")).map(str::to_string))
    })
}

pub fn password_set(app: &AppRef) -> bool {
    !app.config().auth.password_hash.is_empty()
}

/// Logged in as admin (or no password has been set yet).
pub fn is_admin(app: &AppRef, headers: &HeaderMap) -> bool {
    if !password_set(app) {
        return true;
    }
    cookie_token(headers).is_some_and(|t| app.storage.auth_check(&t, now()).unwrap_or(false))
}

/// Matching API key (`apikey` form field or `Authorization: Bearer`).
pub fn apikey_ok(app: &AppRef, headers: &HeaderMap, form_key: Option<&str>) -> bool {
    let key = app.config().api.key;
    if key.is_empty() {
        return false;
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    [form_key, bearer].into_iter().flatten().any(|k| k.eq_ignore_ascii_case(&key))
}

pub fn can_read(app: &AppRef, headers: &HeaderMap) -> bool {
    app.config().auth.guest_read() || is_admin(app, headers) || apikey_ok(app, headers, None)
}

pub fn hash_password(pw: &str) -> Result<String, String> {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let params = argon2::Params::new(8192, 2, 1, None).map_err(|e| e.to_string())?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    a.hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| argon2::Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

pub fn new_session_cookie(app: &AppRef) -> Option<String> {
    let token = random_hex(32);
    app.storage.auth_create(&token, now() + SESSION_TTL).ok()?;
    Some(format!("{SESSION_COOKIE}={token}; Path=/; Max-Age={SESSION_TTL}; HttpOnly; SameSite=Lax"))
}

pub fn logout(app: &AppRef, headers: &HeaderMap) -> String {
    if let Some(t) = cookie_token(headers) {
        let _ = app.storage.auth_delete(&t);
    }
    format!("{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

// ------------------------------------------------------------------ SSE

/// Server-sent event stream. Each event is written as its own HTTP chunk,
/// which the HeaterMeterClient (MeterMonitor/SMUSHI) parser requires.
pub fn event_stream(app: &AppRef, status_only: bool, cors: bool) -> Response<Body> {
    let mut rx = app.device.subscribe();
    let initial = app.device.status_json();
    let stream = async_stream(move |tx| async move {
        if !status_only {
            let _ = tx.send(Bytes::from_static(b"retry: 1000\n\n")).await;
        }
        // Send the current status right away so clients render immediately
        if initial != "{}" {
            let _ = tx.send(frame("hmstatus", &initial, status_only)).await;
        }
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(ev)) => {
                    if status_only && ev.name != "hmstatus" {
                        continue;
                    }
                    if tx.send(frame(ev.name, &ev.data, status_only)).await.is_err() {
                        break;
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => break,
                Err(_) => {
                    if tx.send(Bytes::from_static(b":\n\n")).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut r = Response::new(Body::from_stream(stream.map(Ok::<_, Infallible>)));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    with_cors(r, cors)
}

fn frame(name: &str, data: &str, raw: bool) -> Bytes {
    if raw { Bytes::from(format!("{data}\n")) } else { Bytes::from(format!("event: {name}\ndata: {data}\n\n")) }
}

/// Minimal channel-backed stream helper.
fn async_stream<F, Fut>(f: F) -> impl futures_util::Stream<Item = Bytes>
where
    F: FnOnce(tokio::sync::mpsc::Sender<Bytes>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    tokio::spawn(f(tx));
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// Stream lines from an unbounded receiver as a text/plain body.
pub fn line_stream(rx: tokio::sync::mpsc::UnboundedReceiver<String>) -> Response<Body> {
    let s =
        tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|l| Ok::<_, Infallible>(Bytes::from(l + "\n")));
    let mut r = Response::new(Body::from_stream(s));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r.headers_mut().insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    r
}

// ------------------------------------------------------------------ static

async fn user_css(State(app): State<AppRef>) -> Response<Body> {
    ([(header::CONTENT_TYPE, "text/css"), (header::CACHE_CONTROL, "no-cache")], app.config().ui.user_css)
        .into_response()
}

async fn static_files(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(f) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return ([(header::CONTENT_TYPE, mime.as_ref()), (header::CACHE_CONTROL, cache)], f.data).into_response();
    }
    if path.starts_with("api/") || path.starts_with("luci/") || path.contains('.') {
        return text(StatusCode::NOT_FOUND, "Not found");
    }
    match Assets::get("index.html") {
        Some(f) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], f.data)
            .into_response(),
        None => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<!doctype html><title>HeaterMeter</title><p>Web UI not built. Run <code>npm run build</code> in <code>heatermeter-neo/web</code>. The legacy view is at <a href=\"/luci/lm/light\">/luci/lm/light</a>.</p>",
        )
            .into_response(),
    }
}
