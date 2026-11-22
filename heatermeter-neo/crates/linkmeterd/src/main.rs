//! linkmeterd: HeaterMeter LinkMeter daemon.

mod alarms;
mod app;
mod bootconfig;
mod config;
mod device;
mod discovery;
mod firmware;
mod http;
mod notify;
mod storage;
mod wifi;

use app::{App, AppRef, settings_of};
use config::Config;
use device::{Device, SerialOptions};
use std::path::PathBuf;
use std::sync::Arc;

const USAGE: &str = "linkmeterd - HeaterMeter LinkMeter daemon

USAGE:
  linkmeterd [OPTIONS]                 run the daemon
  linkmeterd import-uci <FILE>         import an OpenWrt /etc/config/linkmeter file
  linkmeterd set-password              set the admin password (read from stdin)
  linkmeterd version

OPTIONS:
  -c, --config <FILE>    configuration file [default: /etc/linkmeter/config.toml]
  -l, --listen <ADDR>    HTTP listen address (overrides config)
  -d, --data-dir <DIR>   data directory (overrides config)
      --simulate         use the built-in HeaterMeter simulator
      --sim-speed <N>    simulator speed-up factor
";

struct Args {
    config: PathBuf,
    listen: Option<String>,
    data_dir: Option<PathBuf>,
    simulate: bool,
    sim_speed: Option<f64>,
    command: Option<String>,
    rest: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        config: "/etc/linkmeter/config.toml".into(),
        listen: None,
        data_dir: None,
        simulate: false,
        sim_speed: None,
        command: None,
        rest: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-c" | "--config" => a.config = val("--config")?.into(),
            "-l" | "--listen" => a.listen = Some(val("--listen")?),
            "-d" | "--data-dir" => a.data_dir = Some(val("--data-dir")?.into()),
            "--simulate" => a.simulate = true,
            "--sim-speed" => a.sim_speed = Some(val("--sim-speed")?.parse().map_err(|_| "bad --sim-speed")?),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if a.command.is_none() => a.command = Some(s.to_string()),
            s => a.rest.push(s.to_string()),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,hyper=warn".into()),
        )
        .with_target(false)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut cfg = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot load config: {e}");
            std::process::exit(1);
        }
    };

    match args.command.as_deref() {
        Some("version") => {
            println!("linkmeterd {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("import-uci") => {
            let Some(file) = args.rest.first() else {
                eprintln!("usage: linkmeterd import-uci <FILE>");
                std::process::exit(2);
            };
            let text = std::fs::read_to_string(file).unwrap_or_else(|e| {
                eprintln!("{file}: {e}");
                std::process::exit(1)
            });
            let cfg = config::import_uci(&text, cfg);
            if let Err(e) = cfg.save(&args.config) {
                eprintln!("cannot write {}: {e}", args.config.display());
                std::process::exit(1);
            }
            println!("imported {file} into {}", args.config.display());
            return;
        }
        Some("set-password") => {
            let mut pw = String::new();
            eprint!("New admin password: ");
            if std::io::stdin().read_line(&mut pw).is_err() || pw.trim().len() < 6 {
                eprintln!("password must be at least 6 characters");
                std::process::exit(1);
            }
            cfg.auth.password_hash = http::hash_password(pw.trim()).expect("hash");
            if let Err(e) = cfg.save(&args.config) {
                eprintln!("cannot write {}: {e}", args.config.display());
                std::process::exit(1);
            }
            println!("password updated");
            return;
        }
        Some(other) => {
            eprintln!("unknown command {other}\n\n{USAGE}");
            std::process::exit(2);
        }
        None => {}
    }

    if args.simulate {
        cfg.daemon.serial_device = "sim".into();
        cfg.firmware.enabled = false;
        cfg.wifi.enabled = false;
    }
    if let Some(s) = args.sim_speed {
        cfg.daemon.sim_speed = s;
    }
    if let Some(l) = args.listen {
        cfg.daemon.listen = l;
    }
    if let Some(d) = args.data_dir {
        cfg.daemon.data_dir = d;
    }

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    rt.block_on(run(cfg, args.config));
}

async fn run(cfg: Config, config_path: PathBuf) {
    let listen = cfg.daemon.listen.clone();
    let app = match setup(cfg, config_path).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("linkmeterd {} listening on http://{listen}", env!("CARGO_PKG_VERSION"));
    let server =
        axum::serve(listener, http::router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>());
    tokio::select! {
        r = server => {
            if let Err(e) = r {
                tracing::error!("http server: {e}");
            }
        }
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    let _ = app.storage.flush();
}

/// Open storage, build the app state and start all background tasks.
async fn setup(mut cfg: Config, config_path: PathBuf) -> Result<AppRef, String> {
    std::fs::create_dir_all(&cfg.daemon.data_dir)
        .map_err(|e| format!("cannot create data dir {}: {e}", cfg.daemon.data_dir.display()))?;
    let storage = storage::Storage::open(&cfg.daemon.data_dir.join("history.db"), cfg.daemon.session_gap_hours)
        .map_err(|e| format!("cannot open database: {e}"))?;
    if cfg.api.key.is_empty() {
        cfg.api.key = app::random_hex(16);
        if let Err(e) = cfg.save(&config_path) {
            tracing::warn!("cannot save config {}: {e}", config_path.display());
        }
    }
    let uuid = discovery::load_uuid(&cfg.daemon.data_dir);

    let (sys_tx, sys_rx) = tokio::sync::mpsc::unbounded_channel();
    let (device, tasks) = Device::new(settings_of(&cfg), cfg.ramp.clone(), storage.clone(), sys_tx);
    let app: AppRef = Arc::new(App::new(cfg.clone(), config_path, device.clone(), storage.clone(), uuid));

    {
        let app = app.clone();
        tokio::spawn(async move {
            for line in bootconfig::apply(&app).await {
                tracing::info!(target: "bootconfig", "{line}");
            }
        });
    }
    tokio::spawn(alarms::run(app.clone(), sys_rx));
    tokio::spawn(discovery::ipwatch_task(app.clone()));
    if cfg.daemon.ssdp {
        tokio::spawn(discovery::ssdp_task(app.clone()));
    }
    if cfg.wifi.enabled {
        tokio::spawn(wifi::manager_task(app.clone()));
    }
    {
        let storage = storage.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                t.tick().await;
                let s = storage.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = s.flush() {
                        tracing::error!("history flush failed: {e}");
                    }
                })
                .await;
            }
        });
    }

    let recover: Option<device::RecoverFn> = if cfg.firmware.enabled && cfg.firmware.auto_recover {
        let app = app.clone();
        Some(Arc::new(move || {
            let app = app.clone();
            Box::pin(firmware::recover(app)) as futures_util::future::BoxFuture<'static, bool>
        }))
    } else {
        None
    };
    let opts = SerialOptions {
        device: cfg.daemon.serial_device.clone(),
        baud: cfg.daemon.serial_baud,
        chunked: cfg.daemon.serial_chunked,
        log: cfg.daemon.serial_log.clone(),
        sim_speed: cfg.daemon.sim_speed,
    };
    tokio::spawn(device::run(device.clone(), tasks, opts, recover));
    Ok(app)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod integration {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn start() -> (AppRef, std::net::SocketAddr, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.daemon.serial_device = "sim".into();
        cfg.daemon.data_dir = dir.path().join("data");
        cfg.daemon.ssdp = false;
        cfg.firmware.enabled = false;
        cfg.wifi.enabled = false;
        let app = setup(cfg, dir.path().join("config.toml")).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, http::router(app.clone())).into_future());
        // wait for the simulator handshake and a first status
        for _ in 0..50 {
            if app.device.status_json() != "{}" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        (app, addr, dir)
    }

    /// HeaterMeterClient (MeterMonitor/SMUSHI) sends a bare HTTP/1.1 request
    /// without a Host header and parses chunked SSE where each chunk holds
    /// complete lines.
    #[tokio::test(flavor = "multi_thread")]
    async fn heatermeterclient_stream_compat() {
        let (_app, addr, _dir) = start().await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /luci/lm/stream HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(500), s.read(&mut tmp)).await {
                Ok(Ok(n)) if n > 0 => buf.extend_from_slice(&tmp[..n]),
                _ => {}
            }
            if String::from_utf8_lossy(&buf).matches("event: hmstatus").count() >= 2 {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let (head, body) = text.split_once("\r\n\r\n").expect("headers");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"));
        assert!(head.to_ascii_lowercase().contains("text/event-stream"));
        // Decode chunks and check every chunk ends on a line boundary
        let mut rest = body;
        let mut events = Vec::new();
        while let Some((size, after)) = rest.split_once("\r\n") {
            let Ok(n) = usize::from_str_radix(size.trim(), 16) else { break };
            if n == 0 || after.len() < n + 2 {
                break;
            }
            let chunk = &after[..n];
            assert!(chunk.ends_with('\n'), "chunk must end with a newline: {chunk:?}");
            events.push(chunk.to_string());
            rest = &after[n + 2..];
        }
        assert_eq!(events[0], "retry: 1000\n\n");
        let status = events.iter().find(|e| e.starts_with("event: hmstatus\n")).expect("hmstatus event");
        let json = status.trim_end().strip_prefix("event: hmstatus\ndata: ").unwrap();
        assert!(json.len() < 1024, "status must fit the client's 1024 byte buffer");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(v["temps"].as_array().unwrap().len() == 4);
        assert!(v["fan"]["c"].is_number());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn captive_portal_redirects_ap_clients() {
        use tower::ServiceExt;
        let (app, _addr, _dir) = start().await;
        let req = |ip: &str, host: &str| {
            let mut r = axum::http::Request::builder()
                .uri("/generate_204")
                .header("host", host)
                .body(axum::body::Body::empty())
                .unwrap();
            r.extensions_mut()
                .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(format!("{ip}:5000").parse().unwrap()));
            r
        };
        // AP not running: nothing special
        let r =
            http::router(app.clone()).oneshot(req("192.168.201.10", "connectivitycheck.gstatic.com")).await.unwrap();
        assert_eq!(r.status(), 200);
        app.wifi.set_ap_active(true);
        // phone on the AP asking for another host is sent to the device
        let r =
            http::router(app.clone()).oneshot(req("192.168.201.10", "connectivitycheck.gstatic.com")).await.unwrap();
        assert_eq!(r.status(), 307);
        assert_eq!(r.headers()["location"], "http://192.168.201.1/");
        // requests to the device itself are served
        let r = http::router(app.clone()).oneshot(req("192.168.201.10", "192.168.201.1")).await.unwrap();
        assert_eq!(r.status(), 200);
        // LAN clients are never redirected
        let r = http::router(app.clone()).oneshot(req("192.168.1.10", "grill.example")).await.unwrap();
        assert_eq!(r.status(), 200);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn boot_config_admin_password_and_ap() {
        let (app, _addr, dir) = start().await;
        let file = dir.path().join("heatermeter.txt");
        std::fs::write(
            &file,
            "# test\nadmin_password = grillimestari\nap_ssid = Savustin\nap_password = short\nunits = C\n",
        )
        .unwrap();
        let log = bootconfig::apply_file(&app, &file).await;
        assert!(log.iter().any(|l| l.contains("admin password set")), "{log:?}");
        assert!(log.iter().any(|l| l.contains("ap_password: must be 8-63")), "{log:?}");
        let cfg = app.config();
        assert!(http::verify_password("grillimestari", &cfg.auth.password_hash));
        assert_eq!(cfg.wifi.ap_ssid, "Savustin");
        assert_eq!(app.take_pending_units(), Some('C'));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("grillimestari"), "{text}");
        assert!(text.contains("# admin_password: asetettu"));
        // applying again re-applies nothing; the invalid AP password is still reported
        let log = bootconfig::apply_file(&app, &file).await;
        assert_eq!(log, vec!["ap_password: must be 8-63 characters, ignored".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_endpoints() {
        let (app, addr, _dir) = start().await;
        let base = format!("http://{addr}");
        let c = reqwest::Client::new();
        let st: serde_json::Value =
            c.get(format!("{base}/luci/lm/hmstatus")).send().await.unwrap().json().await.unwrap();
        assert!(st["time"].is_number());
        let conf: serde_json::Value = c.get(format!("{base}/luci/lm/conf")).send().await.unwrap().json().await.unwrap();
        assert_eq!(conf["ucid"], hm_sim::VERSION);
        let v: serde_json::Value =
            c.get(format!("{base}/luci/lm/api/version")).send().await.unwrap().json().await.unwrap();
        assert_eq!(v["api"], 1);
        let pit = c.get(format!("{base}/luci/lm/api/status/temps/0/n")).send().await.unwrap().text().await.unwrap();
        assert_eq!(pit, "Pit");
        // write without key is refused, with key accepted
        let r = c.post(format!("{base}/luci/lm/api/config")).form(&[("sp", "250")]).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let key = app.config().api.key;
        let r =
            c.post(format!("{base}/luci/lm/api/config")).form(&[("sp", "250"), ("apikey", &key)]).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.text().await.unwrap().contains("sp to 250 = OK"));
        // the simulator applies the setpoint
        let mut ok = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if app.device.read(|c| c.status.set.clone()) == "250" {
                ok = true;
                break;
            }
        }
        assert!(ok, "setpoint not applied");
        app.storage.flush().unwrap();
        let hist = c.get(format!("{base}/luci/lm/hist")).send().await.unwrap().text().await.unwrap();
        assert!(hist.lines().count() >= 1, "{hist}");
    }
}
