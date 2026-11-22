//! Serial bridge to HeaterMeter: owns the [`Core`], the serial port (or the
//! built-in simulator) and turns core [`Effect`]s into real actions.

use crate::storage::Storage;
use hm_protocol::Command;
use lm_core::{AlarmEvent, Core, Effect, LogLevel, Settings, plugins::RampParams};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};

/// A broadcast event (SSE).
#[derive(Debug, Clone)]
pub struct Event {
    pub name: &'static str,
    pub data: Arc<str>,
}

/// Notifications for the rest of the daemon.
#[derive(Debug, Clone)]
pub enum SysEvent {
    Alarm(AlarmEvent),
    ResetConfig,
    RampStarted(f64),
    RampCancelled,
    ConfigComplete,
    /// Start (`true`) or stop the wifi access point (LCD network menu)
    WifiAp(bool),
}

enum Control {
    Pause(oneshot::Sender<()>),
    Resume,
}

pub struct SerialOptions {
    pub device: String,
    pub baud: u32,
    pub chunked: bool,
    pub log: Option<PathBuf>,
    pub sim_speed: f64,
}

pub struct Device {
    core: Mutex<Core>,
    events: broadcast::Sender<Event>,
    cmd_tx: mpsc::UnboundedSender<Command>,
    control: mpsc::Sender<Control>,
    sys_tx: mpsc::UnboundedSender<SysEvent>,
    storage: Storage,
    connected: AtomicBool,
    port: Mutex<Option<String>>,
}

pub struct DeviceTasks {
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    control_rx: mpsc::Receiver<Control>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Device {
    pub fn new(
        settings: Settings,
        ramp: RampParams,
        storage: Storage,
        sys_tx: mpsc::UnboundedSender<SysEvent>,
    ) -> (Arc<Device>, DeviceTasks) {
        let (events, _) = broadcast::channel(512);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (control, control_rx) = mpsc::channel(4);
        let dev = Arc::new(Device {
            core: Mutex::new(Core::new(settings, ramp)),
            events,
            cmd_tx,
            control,
            sys_tx,
            storage,
            connected: AtomicBool::new(false),
            port: Mutex::new(None),
        });
        (dev, DeviceTasks { cmd_rx, control_rx })
    }

    fn core(&self) -> std::sync::MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run a closure on the core and apply the returned effects.
    pub fn with_core<R>(&self, f: impl FnOnce(&mut Core) -> (R, Vec<Effect>)) -> R {
        let (r, fx) = {
            let mut c = self.core();
            f(&mut c)
        };
        self.apply(fx);
        r
    }

    /// Read-only access to the core.
    pub fn read<R>(&self, f: impl FnOnce(&Core) -> R) -> R {
        f(&self.core())
    }

    pub fn apply(&self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Serial(cmd) => {
                    let _ = self.cmd_tx.send(cmd);
                }
                Effect::Broadcast { name, data } => {
                    let _ = self.events.send(Event { name, data: data.into() });
                }
                Effect::Sample(s) => self.storage.add(s),
                Effect::AlarmStart(a) => {
                    let _ = self.sys_tx.send(SysEvent::Alarm(a));
                }
                Effect::ResetConfig => {
                    let _ = self.sys_tx.send(SysEvent::ResetConfig);
                }
                Effect::RampStarted { start_setpoint } => {
                    let _ = self.sys_tx.send(SysEvent::RampStarted(start_setpoint));
                }
                Effect::RampCancelled => {
                    let _ = self.sys_tx.send(SysEvent::RampCancelled);
                }
                Effect::WifiAp { start } => {
                    let _ = self.sys_tx.send(SysEvent::WifiAp(start));
                }
                Effect::ConfigComplete => {
                    let json = self.read(|c| c.config_json());
                    self.storage.set_config_snapshot(json);
                    let _ = self.sys_tx.send(SysEvent::ConfigComplete);
                }
                Effect::Log(level, msg) => match level {
                    LogLevel::Info => tracing::info!("{msg}"),
                    LogLevel::Warning => tracing::warn!("{msg}"),
                    LogLevel::Error => tracing::error!("{msg}"),
                },
            }
        }
    }

    // ------------------------------------------------------------ API

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.core().unthrottle();
        self.events.subscribe()
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn port(&self) -> Option<String> {
        self.port.lock().ok().and_then(|p| p.clone())
    }

    pub fn status_json(&self) -> String {
        self.read(|c| c.status_json())
    }

    pub fn config_json(&self) -> String {
        self.read(|c| c.config_json())
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.with_core(|c| match c.set(key, value) {
            Ok(fx) => (Ok(()), fx),
            Err(e) => (Err(e), Vec::new()),
        })
    }

    pub fn reboot(&self) {
        self.with_core(|c| ((), c.reboot()));
    }

    pub fn toast(&self, l1: &str, l2: &str) {
        self.with_core(|c| ((), c.toast(l1, l2)));
    }

    pub fn alarm_test(&self, probe: usize, kind: Option<char>, thresh: Option<&str>) {
        self.with_core(|c| ((), c.alarm_test(probe, kind, thresh)));
    }

    pub fn set_settings(&self, s: Settings) {
        self.core().settings = s;
    }

    pub fn reload_ramp(&self, p: RampParams) {
        self.core().ramp.reload(p);
    }

    /// Stop using the serial port (for AVR flashing). Resolves once the port
    /// is closed.
    pub async fn pause(&self) {
        let (tx, rx) = oneshot::channel();
        if self.control.send(Control::Pause(tx)).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
        }
    }

    pub async fn resume(&self) {
        let _ = self.control.send(Control::Resume).await;
    }
}

fn auto_device() -> String {
    for p in ["/dev/serial0", "/dev/ttyS0", "/dev/ttyAMA0"] {
        if Path::new(p).exists() {
            return p.into();
        }
    }
    "/dev/serial0".into()
}

type Transport = (Box<dyn AsyncRead + Unpin + Send>, Box<dyn AsyncWrite + Unpin + Send>);

async fn open_transport(opts: &SerialOptions) -> std::io::Result<(Transport, String)> {
    if opts.device == "sim" {
        let (host, sim_side) = tokio::io::duplex(8192);
        tokio::spawn(run_simulator(sim_side, opts.sim_speed));
        let (r, w) = tokio::io::split(host);
        return Ok(((Box::new(r), Box::new(w)), "simulator".into()));
    }
    let path = if opts.device == "auto" { auto_device() } else { opts.device.clone() };
    use tokio_serial::SerialPortBuilderExt;
    let port = tokio_serial::new(&path, opts.baud)
        .data_bits(tokio_serial::DataBits::Eight)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .flow_control(tokio_serial::FlowControl::None)
        .open_native_async()
        .map_err(|e| std::io::Error::other(format!("{path}: {e}")))?;
    let (r, w) = tokio::io::split(port);
    Ok(((Box::new(r), Box::new(w)), path))
}

async fn run_simulator(io: tokio::io::DuplexStream, speed: f64) {
    let mut sim = hm_sim::Simulator::new();
    sim.time_scale = speed.max(0.1);
    let (r, mut w) = tokio::io::split(io);
    let mut r = BufReader::new(r);
    let _ = w.write_all(sim.boot().as_bytes()).await;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut buf = Vec::new();
    loop {
        tokio::select! {
            n = r.read_until(b'\n', &mut buf) => {
                match n {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let out = sim.receive(&String::from_utf8_lossy(&buf));
                        buf.clear();
                        if w.write_all(out.as_bytes()).await.is_err() { return; }
                    }
                }
            }
            _ = tick.tick() => {
                if w.write_all(sim.tick().as_bytes()).await.is_err() { return; }
            }
        }
    }
}

async fn write_cmd(
    w: &mut (dyn AsyncWrite + Unpin + Send),
    cmd: &Command,
    chunked: bool,
    baud: u32,
) -> std::io::Result<()> {
    let data = cmd.wire();
    if chunked {
        // 8-byte chunks: the Pi3/Zero W mini-UART FIFO is 8 bytes deep
        let byte_ns = 1_000_000_000u64 / u64::from(baud.max(1200)) * 10;
        for chunk in data.as_bytes().chunks(8) {
            w.write_all(chunk).await?;
            w.flush().await?;
            tokio::time::sleep(Duration::from_nanos(byte_ns * 8)).await;
        }
    } else {
        w.write_all(data.as_bytes()).await?;
        w.flush().await?;
    }
    Ok(())
}

/// Hook to run AVR recovery when HeaterMeter does not answer.
pub type RecoverFn = Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, bool> + Send + Sync>;

/// Main serial loop. Reopens the port on errors.
pub async fn run(dev: Arc<Device>, mut tasks: DeviceTasks, opts: SerialOptions, recover: Option<RecoverFn>) {
    let mut log = match &opts.log {
        Some(p) => tokio::fs::File::create(p).await.ok(),
        None => None,
    };
    let mut recovered = false;
    let mut last_err = String::new();
    loop {
        let ((reader, mut writer), name) = match open_transport(&opts).await {
            Ok(t) => t,
            Err(e) => {
                let msg = e.to_string();
                if msg != last_err {
                    tracing::warn!("cannot open serial port: {msg}");
                    last_err = msg;
                }
                // still honour pause/resume while waiting
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    Some(c) = tasks.control_rx.recv() => {
                        if let Control::Pause(ack) = c { let _ = ack.send(()); wait_resume(&mut tasks).await; }
                    }
                }
                // discard queued commands while disconnected
                while tasks.cmd_rx.try_recv().is_ok() {}
                continue;
            }
        };
        last_err.clear();
        tracing::info!("connected to HeaterMeter on {name}");
        *dev.port.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.clone());
        dev.core().reset();
        dev.connected.store(true, Ordering::Relaxed);

        let mut reader = BufReader::new(reader);
        // Drain stale input so the /config reply cannot overrun
        if opts.device != "sim" {
            let mut junk = vec![0u8; 1024];
            while let Ok(Ok(n)) =
                tokio::time::timeout(Duration::from_millis(50), tokio::io::AsyncReadExt::read(&mut reader, &mut junk))
                    .await
            {
                if n == 0 {
                    break;
                }
            }
        }
        let started = tokio::time::Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut buf = Vec::with_capacity(256);
        let mut last_write = tokio::time::Instant::now() - Duration::from_secs(1);
        let outcome = loop {
            tokio::select! {
                r = reader.read_until(b'\n', &mut buf) => {
                    match r {
                        Ok(0) => break "eof".to_string(),
                        Err(e) => break e.to_string(),
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf).into_owned();
                            buf.clear();
                            if let Some(f) = log.as_mut() {
                                let _ = f.write_all(line.as_bytes()).await;
                            }
                            let fx = dev.core().on_line(line.trim_end(), now());
                            dev.apply(fx);
                        }
                    }
                }
                Some(cmd) = tasks.cmd_rx.recv() => {
                    // Give HeaterMeter ~100ms between commands
                    let since = last_write.elapsed();
                    if since < Duration::from_millis(100) {
                        tokio::time::sleep(Duration::from_millis(100) - since).await;
                    }
                    if let Some(f) = log.as_mut() {
                        let _ = f.write_all(format!("WRITE:{}\n", cmd.text()).as_bytes()).await;
                    }
                    if let Err(e) = write_cmd(&mut *writer, &cmd, opts.chunked && opts.device != "sim", opts.baud).await {
                        break e.to_string();
                    }
                    last_write = tokio::time::Instant::now();
                }
                _ = tick.tick() => {
                    let fx = dev.core().on_tick(now());
                    dev.apply(fx);
                    if !recovered && started.elapsed() > Duration::from_secs(10) {
                        recovered = true;
                        if !dev.read(|c| c.has_config()) {
                            if recover.is_some() {
                                tracing::warn!("no response from HeaterMeter, running AVR recovery");
                                break "recover".to_string();
                            } else {
                                tracing::warn!("no response from HeaterMeter on {name}");
                            }
                        }
                    }
                }
                Some(c) = tasks.control_rx.recv() => {
                    match c {
                        Control::Pause(ack) => {
                            let _ = ack.send(());
                            break "paused".to_string();
                        }
                        Control::Resume => {}
                    }
                }
            }
        };
        dev.connected.store(false, Ordering::Relaxed);
        drop(reader);
        drop(writer);
        let _ = dev.storage.flush();
        match outcome.as_str() {
            "paused" => {
                tracing::info!("serial paused");
                dev.core().reset();
                wait_resume(&mut tasks).await;
                tracing::info!("serial resumed");
            }
            "recover" => {
                if let Some(rec) = recover.as_ref() {
                    let ok = rec().await;
                    tracing::info!("AVR recovery {}", if ok { "OK" } else { "failed" });
                }
            }
            other => {
                tracing::warn!("serial connection lost: {other}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn wait_resume(tasks: &mut DeviceTasks) {
    while let Some(c) = tasks.control_rx.recv().await {
        match c {
            Control::Resume => break,
            Control::Pause(ack) => {
                let _ = ack.send(());
            }
        }
    }
    while tasks.cmd_rx.try_recv().is_ok() {}
}
