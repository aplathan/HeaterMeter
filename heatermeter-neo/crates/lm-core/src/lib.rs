//! LinkMeter core: a synchronous, I/O free port of `linkmeterd.lua`.
//!
//! Feed serial lines with [`Core::on_line`] and periodic ticks with
//! [`Core::on_tick`]; the core returns [`Effect`]s the daemon must carry out
//! (write to serial, broadcast an SSE event, store a sample, run alarm
//! actions...). Time is always passed in so everything is deterministic and
//! testable.

pub mod fit;
pub mod num;
pub mod peaks;
pub mod plugins;
pub mod status;

use hm_protocol::{
    Command, NUM_PROBES, Segment, StatusUpdate, parse_alarm_field, parse_num, parse_rf_update, validate,
};
use num::{fmt_num, json_num};
use plugins::{Dph, NetInfo, Ramp, RampParams, UnkProbe};
use serde::Serialize;
use status::{StatusState, Throttle, lit};
use std::collections::BTreeMap;

pub use peaks::Peaks;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

/// A configuration value as cached from HeaterMeter.
#[derive(Debug, Clone, PartialEq)]
pub enum CfgVal {
    Num(f64),
    Str(String),
}

impl CfgVal {
    pub fn as_str(&self) -> String {
        match self {
            CfgVal::Num(n) => fmt_num(*n),
            CfgVal::Str(s) => s.clone(),
        }
    }

    fn to_json(&self) -> String {
        match self {
            CfgVal::Num(n) => fmt_num(*n),
            CfgVal::Str(s) if s == "null" => "null".into(),
            CfgVal::Str(s) => serde_json::to_string(s).unwrap_or_default(),
        }
    }
}

pub type ConfigMap = BTreeMap<String, CfgVal>;

/// One recorded data point (1 Hz).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    pub time: i64,
    /// Setpoint; `None` in manual/off mode
    pub sp: Option<f64>,
    pub temps: [Option<f64>; NUM_PROBES],
    /// Output %, or `-lid seconds` while lid mode is active
    pub out: Option<f64>,
    pub fan: Option<f64>,
    pub servo: Option<f64>,
}

/// An alarm that started ringing.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmEvent {
    pub probe: usize,
    /// 'L' or 'H'
    pub kind: char,
    pub threshold: String,
    /// Environment for alarm scripts and message templates: the full
    /// configuration map plus `al_probe`, `al_type`, `al_thresh`, `al_prep`,
    /// `pn` and `pcurr`.
    pub env: BTreeMap<String, String>,
    /// True when triggered via alarm test
    pub test: bool,
}

/// Something the daemon must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Write a command to HeaterMeter.
    Serial(Command),
    /// Broadcast an event to stream listeners. `name == "hmstatus"` carries the status JSON.
    Broadcast {
        name: &'static str,
        data: String,
    },
    /// Store a data point.
    Sample(Sample),
    /// An alarm started ringing (run notifications/actions).
    AlarmStart(AlarmEvent),
    /// HeaterMeter requested a factory reset (`$HMRC`).
    ResetConfig,
    /// Ramp started; persist the start setpoint.
    RampStarted {
        start_setpoint: f64,
    },
    /// Ramp ended or was cancelled; persist `enabled = false`.
    RampCancelled,
    /// All configuration segments have been received.
    ConfigComplete,
    Log(LogLevel, String),
}

/// Daemon-side settings the core needs.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub lidtrack_enabled: bool,
}

/// Context handed to plugins.
pub struct Ctx<'a> {
    pub effects: &'a mut Vec<Effect>,
    pub status: &'a mut StatusState,
    pub config: &'a ConfigMap,
    pub settings: &'a Settings,
    pub throttle: &'a mut Throttle,
}

impl Ctx<'_> {
    pub fn write(&mut self, cmd: Command) {
        self.effects.push(Effect::Serial(cmd));
    }

    pub fn log(&mut self, level: LogLevel, msg: String) {
        self.effects.push(Effect::Log(level, msg));
    }

    pub fn broadcast(&mut self, name: &'static str, data: String) {
        self.effects.push(Effect::Broadcast { name, data });
    }

    pub fn publish_global(&mut self, key: &str, json: Option<String>) {
        self.status.ext.set(key, json);
    }

    pub fn publish_probe(&mut self, probe: usize, key: &str, json: Option<String>) {
        if let Some(p) = self.status.probes.get_mut(probe) {
            p.ext.set(key, json);
        }
    }

    pub fn config_str(&self, k: &str) -> Option<&str> {
        match self.config.get(k) {
            Some(CfgVal::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn config_num(&self, k: &str) -> Option<f64> {
        match self.config.get(k) {
            Some(CfgVal::Num(n)) => Some(*n),
            Some(CfgVal::Str(s)) => parse_num(s),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AlarmState {
    threshold: String,
    ringing: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RfStatus {
    pub id: String,
    pub lobatt: u8,
    pub rssi: i32,
    pub reset: u8,
    pub native: u8,
}

const REQUIRED_CONFIG: [&str; 12] =
    ["UCID", "HMPD", "HMFN", "HMPN", "HMPC,0", "HMPC,1", "HMPC,2", "HMPC,3", "HMPO", "HMLD", "HMLB", "HMAL"];

/// The LinkMeter device state.
pub struct Core {
    /// `None` until the first valid line from HeaterMeter is seen.
    config: Option<ConfigMap>,
    remaining: Option<Vec<&'static str>>,
    last_config_rx: Option<i64>,
    pub status: StatusState,
    throttle: Throttle,
    alarms: [AlarmState; 8],
    rf_map: Vec<String>,
    rf_status: BTreeMap<String, RfStatus>,
    last_update: i64,
    last_sample: i64,
    pub settings: Settings,
    pub peaks: Peaks,
    pub ramp: Ramp,
    pub dph: Dph,
    pub unkprobe: UnkProbe,
    pub netinfo: NetInfo,
    /// Raw `$HMSU` line of the last status update (for diagnostics)
    pub last_status_line: Option<String>,
    pub checksum_errors: u64,
}

impl Core {
    pub fn new(settings: Settings, ramp: RampParams) -> Core {
        Core {
            config: None,
            remaining: Some(REQUIRED_CONFIG.to_vec()),
            last_config_rx: None,
            status: StatusState::default(),
            throttle: Throttle::default(),
            alarms: Default::default(),
            rf_map: Vec::new(),
            rf_status: BTreeMap::new(),
            last_update: 0,
            last_sample: 0,
            settings,
            peaks: Peaks::default(),
            ramp: Ramp::new(ramp),
            dph: Dph::default(),
            unkprobe: UnkProbe::default(),
            netinfo: NetInfo::default(),
            last_status_line: None,
            checksum_errors: 0,
        }
    }

    /// Reset all HeaterMeter derived state (`initHmVars` + `initPlugins`).
    pub fn reset(&mut self) {
        let ramp = self.ramp.params.clone();
        let netinfo = std::mem::take(&mut self.netinfo);
        let settings = self.settings.clone();
        let cerr = self.checksum_errors;
        *self = Core::new(settings, ramp);
        self.netinfo = netinfo;
        self.checksum_errors = cerr;
    }

    pub fn has_config(&self) -> bool {
        self.config.is_some()
    }

    pub fn config_complete(&self) -> bool {
        self.config.is_some() && self.remaining.is_none()
    }

    pub fn missing_config(&self) -> Vec<&'static str> {
        self.remaining.clone().unwrap_or_default()
    }

    pub fn config(&self) -> Option<&ConfigMap> {
        self.config.as_ref()
    }

    pub fn ucid(&self) -> Option<String> {
        self.config.as_ref()?.get("ucid").map(|v| v.as_str())
    }

    // ------------------------------------------------------------ input

    /// Handle one line read from the serial port.
    pub fn on_line(&mut self, raw: &str, now: i64) -> Vec<Effect> {
        let mut fx = Vec::new();
        match validate(raw) {
            Err(hm_protocol::LineError::NoDollar) => {}
            Err(hm_protocol::LineError::BadChecksum) => {
                self.checksum_errors += 1;
                fx.push(Effect::Log(LogLevel::Warning, format!("Checksum failed: {}", raw.trim_end())));
            }
            Ok(v) => {
                if self.config.is_none() {
                    self.config = Some(ConfigMap::new());
                    fx.push(Effect::Serial(Command::Config));
                    self.last_config_rx = Some(now);
                }
                self.dispatch(v.line, now, &mut fx);
            }
        }
        self.check_config_remaining(now, &mut fx);
        fx
    }

    /// Periodic tick (call about once per second).
    pub fn on_tick(&mut self, now: i64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.check_config_remaining(now, &mut fx);
        fx
    }

    fn check_config_remaining(&mut self, now: i64, fx: &mut Vec<Effect>) {
        let (Some(rem), Some(last)) = (&self.remaining, self.last_config_rx) else { return };
        if now - last > 2 {
            fx.push(Effect::Log(LogLevel::Warning, format!("Requesting config retransmit for {}", rem.join(","))));
            fx.push(Effect::Serial(Command::Config));
            self.last_config_rx = Some(now);
        }
    }

    fn config_received(&mut self, seg: &str, now: i64, fx: &mut Vec<Effect>) {
        let Some(rem) = self.remaining.as_mut() else { return };
        self.last_config_rx = Some(now);
        rem.retain(|s| *s != seg);
        if rem.is_empty() {
            self.remaining = None;
            self.last_config_rx = None;
            fx.push(Effect::ConfigComplete);
        }
    }

    fn set_cfg(&mut self, fields: &[&str], names: &[&str], numeric: bool) {
        let Some(cfg) = self.config.as_mut() else { return };
        for (i, name) in names.iter().enumerate() {
            let Some(v) = fields.get(i) else { break };
            if name.is_empty() {
                continue;
            }
            let val = if numeric {
                parse_num(v).map(CfgVal::Num).unwrap_or_else(|| CfgVal::Str(v.to_string()))
            } else {
                CfgVal::Str(v.to_string())
            };
            cfg.insert(name.to_string(), val);
        }
    }

    fn dispatch(&mut self, line: &str, now: i64, fx: &mut Vec<Effect>) {
        let seg = Segment::parse(line);
        let f = &seg.fields;
        match seg.tag {
            "HMSU" => self.seg_state_update(line, f, now, fx),
            "HMPN" => {
                self.config_received("HMPN", now, fx);
                self.set_cfg(f, &["pn0", "pn1", "pn2", "pn3"], false);
                for (i, v) in f.iter().take(NUM_PROBES).enumerate() {
                    self.status.probes[i].name = v.to_string();
                }
            }
            "HMPO" => {
                self.config_received("HMPO", now, fx);
                self.set_cfg(f, &["po0", "po1", "po2", "po3"], true);
            }
            "HMPD" => {
                self.config_received("HMPD", now, fx);
                self.set_cfg(f, &["", "pidp", "pidi", "pidd", "u"], true);
            }
            "HMLD" => {
                self.config_received("HMLD", now, fx);
                self.set_cfg(f, &["lo", "ld"], true);
            }
            "HMFN" => {
                self.config_received("HMFN", now, fx);
                self.set_cfg(f, &["fmin", "fmax", "smin", "smax", "oflag", "fsmax", "fflor", "sceil"], true);
            }
            "HMPC" => {
                let i = f.first().copied().unwrap_or("0");
                if let Some(seg) = ["HMPC,0", "HMPC,1", "HMPC,2", "HMPC,3"].iter().find(|s| s.ends_with(i)) {
                    self.config_received(seg, now, fx);
                }
                let names = [
                    String::new(),
                    format!("pca{i}"),
                    format!("pcb{i}"),
                    format!("pcc{i}"),
                    format!("pcr{i}"),
                    format!("pt{i}"),
                ];
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                self.set_cfg(f, &names, false);
                if let Some(cfg) = self.config.as_mut() {
                    let k = format!("pcr{i}");
                    let n = cfg.get(&k).and_then(|v| parse_num(&v.as_str()));
                    match n {
                        Some(n) => cfg.insert(k, CfgVal::Num(n)),
                        None => cfg.remove(&k),
                    };
                }
            }
            "HMLB" => {
                self.config_received("HMLB", now, fx);
                self.set_cfg(f, &["lb", "lbn", "le0", "le1", "le2", "le3"], false);
            }
            "HMAL" => self.seg_alarm_limits(f, now, fx),
            "HMAR" => {
                let vals: Vec<String> = f.iter().map(|v| lit(v)).collect();
                self.status.ext.set("adc", Some(format!("[{}]", vals.join(","))));
            }
            "HMRF" => {
                self.rf_status.clear();
                for n in parse_rf_update(f) {
                    let st = RfStatus {
                        id: n.id.clone(),
                        lobatt: n.lobatt as u8,
                        rssi: n.rssi,
                        reset: n.reset as u8,
                        native: n.native as u8,
                    };
                    if n.id != "255" {
                        self.rf_status.insert("127".into(), RfStatus { id: "127".into(), ..st.clone() });
                    }
                    self.rf_status.insert(n.id, st);
                }
                self.rf_refresh();
            }
            "HMRM" => {
                self.rf_map = f.iter().map(|s| s.to_string()).collect();
                if let Some(cfg) = self.config.as_mut() {
                    for (i, s) in f.iter().enumerate() {
                        cfg.insert(format!("prfn{i}"), CfgVal::Str(s.to_string()));
                    }
                }
                self.rf_refresh();
            }
            "UCID" => {
                self.config_received("UCID", now, fx);
                if let (Some(v), Some(cfg)) = (f.get(1), self.config.as_mut()) {
                    cfg.insert("ucid".into(), CfgVal::Str(v.to_string()));
                }
            }
            "HMLG" => {
                let msg = f.join(",");
                fx.push(Effect::Broadcast { name: "log", data: serde_json::json!({ "msg": msg }).to_string() });
            }
            "HMND" => {
                let d = f.first().copied().unwrap_or("");
                fx.push(Effect::Broadcast { name: "noisedump", data: serde_json::to_string(d).unwrap_or_default() });
            }
            "HMPS" => {
                let g = |i: usize| f.get(i).map(|v| lit(v)).unwrap_or_else(|| "null".into());
                fx.push(Effect::Broadcast {
                    name: "pidint",
                    data: format!("{{\"b\":{},\"p\":{},\"i\":{},\"d\":{},\"t\":{}}}", g(0), g(1), g(2), g(3), g(4)),
                });
            }
            "HMHI" => {
                let n = |i: usize| f.get(i).and_then(|v| v.trim().parse::<u8>().ok());
                if let (Some(opaque), Some(topic), Some(btn)) = (n(0), n(1), n(2)) {
                    let mut ctx = Ctx {
                        effects: fx,
                        status: &mut self.status,
                        config: self.config.as_ref().unwrap_or(&EMPTY),
                        settings: &self.settings,
                        throttle: &mut self.throttle,
                    };
                    self.netinfo.on_interactive(topic, opaque, btn, &mut ctx);
                }
            }
            "HMRC" => {
                fx.push(Effect::Serial(Command::set("tt", "Resetting,configuration...")));
                fx.push(Effect::ResetConfig);
            }
            _ => {}
        }
    }

    fn rf_refresh(&mut self) {
        for i in 0..NUM_PROBES {
            let val = match self.rf_map.get(i).map(String::as_str) {
                Some("") | None => None,
                Some(src) => Some(match self.rf_status.get(src) {
                    Some(s) => format!("{{\"s\":{},\"b\":{}}}", s.rssi, s.lobatt),
                    None => "0".into(),
                }),
            };
            self.status.probes[i].ext.set("rf", val);
        }
    }

    fn seg_state_update(&mut self, line: &str, f: &[&str], now: i64, fx: &mut Vec<Effect>) {
        let st = StatusUpdate::from_fields(f);
        // Plugin status listeners run before throttling, once config is complete
        if let Some(st) = &st
            && self.remaining.is_none()
            && self.config.is_some()
        {
            let mut ctx = Ctx {
                effects: fx,
                status: &mut self.status,
                config: self.config.as_ref().unwrap_or(&EMPTY),
                settings: &self.settings,
                throttle: &mut self.throttle,
            };
            self.unkprobe.on_status(st);
            self.peaks.on_status(now, st, f, &mut ctx);
            self.dph.on_status(now, st, &mut ctx);
            self.ramp.on_status(st, &mut ctx);
        }

        let Some(st) = st else { return };
        // Every update is recorded (1 Hz); only the broadcast is throttled
        if now != self.last_sample {
            self.last_sample = now;
            let lid = st.lid.unwrap_or(0.0);
            fx.push(Effect::Sample(Sample {
                time: now,
                sp: st.setpoint_num().filter(|v| *v > 0.0),
                temps: st.temps,
                out: if lid != 0.0 { Some(-lid) } else { st.output },
                fan: st.fan,
                servo: st.servo,
            }));
        }

        if self.throttle.should_skip(line) {
            return;
        }
        if now == self.last_update {
            fx.push(Effect::Log(LogLevel::Info, "Discarding duplicate update".into()));
            return;
        }
        self.last_update = now;

        // Manual mode: setpoint '-' becomes -output
        let manual_sp = if st.is_manual() { Some(format!("-{}", f.get(5).copied().unwrap_or("0"))) } else { None };
        let mut fields: Vec<&str> = f.to_vec();
        if let Some(sp) = manual_sp.as_deref() {
            fields[0] = sp;
        }
        self.status.apply(now, &fields);
        self.last_status_line = Some(line.to_string());
        fx.push(Effect::Broadcast { name: "hmstatus", data: self.status.to_json() });
    }

    fn seg_alarm_limits(&mut self, f: &[&str], now: i64, fx: &mut Vec<Effect>) {
        self.config_received("HMAL", now, fx);
        for (i, v) in f.iter().take(8).enumerate() {
            let a = parse_alarm_field(v);
            let probe = i / 2;
            let kind = if i % 2 == 0 { 'L' } else { 'H' };
            let p = &mut self.status.probes[probe];
            if kind == 'L' {
                p.alarm_lo = lit(&a.raw);
            } else {
                p.alarm_hi = lit(&a.raw);
            }
            let was_ringing = self.alarms[i].ringing.is_some();
            let have_ucid = self.config.as_ref().is_some_and(|c| c.contains_key("ucid"));
            if a.ringing && !was_ringing && have_ucid {
                self.alarms[i].ringing = Some(now);
                self.alarms[i].threshold = a.raw.clone();
                self.broadcast_alarm(probe, Some(kind), &a.raw, false, fx);
            } else if !a.ringing && was_ringing {
                self.alarms[i].ringing = None;
                self.broadcast_alarm(probe, None, &a.raw, false, fx);
            }
            self.alarms[i].threshold = a.raw;
        }
    }

    fn broadcast_alarm(&mut self, probe: usize, kind: Option<char>, thresh: &str, test: bool, fx: &mut Vec<Effect>) {
        let pname = self.status.probes[probe].name.clone();
        let cur = self.status.probes[probe].temp.clone();
        match kind {
            Some(k) => {
                fx.push(Effect::Log(LogLevel::Warning, format!("Alarm {probe}{k} started ringing")));
                let mut env: BTreeMap<String, String> =
                    self.config_map().into_iter().map(|(k, v)| (k, v.as_str())).collect();
                env.insert("al_probe".into(), probe.to_string());
                env.insert("al_type".into(), k.to_string());
                env.insert("al_thresh".into(), thresh.to_string());
                env.insert("al_prep".into(), if k == 'H' { "above" } else { "below" }.into());
                if let Some(v) = env.get(&format!("pn{probe}")).cloned() {
                    env.insert("pn".into(), v);
                }
                if let Some(v) = env.get(&format!("pcurr{probe}")).cloned() {
                    env.insert("pcurr".into(), v);
                }
                fx.push(Effect::AlarmStart(AlarmEvent { probe, kind: k, threshold: thresh.into(), env, test }));
            }
            None => fx.push(Effect::Log(LogLevel::Warning, "Alarm stopped".into())),
        }
        self.throttle.unthrottle();
        self.status.probes[probe].ringing = kind;
        let atype = kind.map(|k| format!("\"{k}\"")).unwrap_or_else(|| "null".into());
        fx.push(Effect::Broadcast {
            name: "alarm",
            data: format!(
                "{{\"atype\":{},\"p\":{},\"pn\":{},\"c\":{},\"t\":{}}}",
                atype,
                probe,
                serde_json::to_string(&pname).unwrap_or_default(),
                cur,
                lit(thresh)
            ),
        });
    }

    // ------------------------------------------------------------ host commands

    /// `$LMST,k,v`: send `/set?k=v`.
    pub fn set(&mut self, key: &str, value: &str) -> Result<Vec<Effect>, String> {
        if !hm_protocol::is_valid_set_key(key) {
            return Err(format!("invalid key {key:?}"));
        }
        if value.contains(['\n', '\r']) {
            return Err("invalid value".into());
        }
        let cmd = Command::set(key, value);
        if !cmd.fits() {
            return Err("value too long".into());
        }
        self.throttle.unthrottle();
        Ok(vec![Effect::Serial(cmd)])
    }

    /// `$LMRB`: reboot HeaterMeter and re-request config afterwards.
    pub fn reboot(&mut self) -> Vec<Effect> {
        self.reset();
        vec![Effect::Serial(Command::Reboot)]
    }

    /// `$LMTT`: LCD toast.
    pub fn toast(&self, line1: &str, line2: &str) -> Vec<Effect> {
        let clean = |s: &str| s.replace([',', '\n', '\r'], " ");
        let mut l1 = clean(line1);
        let mut l2 = clean(line2);
        l1.truncate(16);
        l2.truncate(16);
        vec![Effect::Serial(Command::set("tt", format!("{l1},{l2}")))]
    }

    /// Update the IP shown on the LCD (ipwatch). Emits a toast on change.
    pub fn set_ip(&mut self, ip: Option<String>) -> Vec<Effect> {
        if ip.is_none() || ip == self.netinfo.ip {
            return Vec::new();
        }
        self.netinfo.ip = ip.clone();
        let ip = ip.unwrap_or_default();
        if let Some(cfg) = self.config.as_mut() {
            cfg.insert("ip".into(), CfgVal::Str(ip.clone()));
        }
        self.toast("Network Address", &ip)
    }

    /// `$LMAT`: simulate an alarm (`kind = None` simulates stop).
    pub fn alarm_test(&mut self, probe: usize, kind: Option<char>, thresh: Option<&str>) -> Vec<Effect> {
        if probe >= NUM_PROBES {
            return Vec::new();
        }
        let thresh =
            thresh.map(str::to_string).unwrap_or_else(|| fmt_num(self.status.temp(probe).unwrap_or(0.0).floor()));
        let mut fx = Vec::new();
        self.broadcast_alarm(probe, kind, &thresh, true, &mut fx);
        fx
    }

    /// Run a closure with a plugin context (used for unkprobe start/stop).
    pub fn with_ctx<R>(&mut self, f: impl FnOnce(&mut PluginRefs, &mut Ctx) -> R) -> (R, Vec<Effect>) {
        let mut fx = Vec::new();
        let mut refs = PluginRefs { unkprobe: &mut self.unkprobe, ramp: &mut self.ramp };
        let mut ctx = Ctx {
            effects: &mut fx,
            status: &mut self.status,
            config: self.config.as_ref().unwrap_or(&EMPTY),
            settings: &self.settings,
            throttle: &mut self.throttle,
        };
        let r = f(&mut refs, &mut ctx);
        (r, fx)
    }

    /// Force the next status updates out unthrottled (new stream client).
    pub fn unthrottle(&mut self) {
        self.throttle.unthrottle();
    }

    // ------------------------------------------------------------ queries

    /// `$LMSU`
    pub fn status_json(&self) -> String {
        self.status.to_json()
    }

    /// Configuration map incl. derived values (`buildConfigMap`).
    pub fn config_map(&self) -> ConfigMap {
        let Some(cfg) = &self.config else { return ConfigMap::new() };
        let mut r = cfg.clone();
        if self.status.has_update() {
            for i in 0..NUM_PROBES {
                if let Some(t) = self.status.temp(i) {
                    r.insert(format!("pcurr{i}"), CfgVal::Num(t));
                }
            }
            r.insert("sp".into(), CfgVal::Str(self.status.set.clone()));
        }
        for (i, a) in self.alarms.iter().enumerate() {
            if let Some(v) = parse_num(&a.threshold) {
                let kind = if i % 2 == 0 { "l" } else { "h" };
                r.insert(format!("pal{kind}{}", i / 2), CfgVal::Num(v));
            }
        }
        r.insert("cerr".into(), CfgVal::Num(self.checksum_errors as f64));
        r
    }

    /// `$LMCF` JSON
    pub fn config_json(&self) -> String {
        if self.config.is_none() {
            return "{}".into();
        }
        let cm = self.config_map();
        let mut parts = Vec::new();
        for i in 0..NUM_PROBES {
            if !cm.contains_key(&format!("pcurr{i}")) {
                parts.push(format!("\"pcurr{i}\":null"));
            }
        }
        for (k, v) in &cm {
            parts.push(format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), v.to_json()));
        }
        format!("{{{}}}", parts.join(","))
    }

    /// `$LMRF` JSON
    pub fn rf_json(&self) -> String {
        let items: Vec<String> = self
            .rf_status
            .values()
            .map(|s| {
                format!(
                    "{{\"id\":{},\"lobatt\":{},\"rssi\":{},\"reset\":{},\"native\":{}}}",
                    lit(&s.id),
                    s.lobatt,
                    s.rssi,
                    s.reset,
                    s.native
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// `$LMGT,k,def,...`
    pub fn get(&self, pairs: &[(&str, &str)]) -> Vec<String> {
        let cm = self.config_map();
        pairs.iter().map(|(k, d)| cm.get(*k).map(|v| v.as_str()).unwrap_or_else(|| d.to_string())).collect()
    }

    /// Alarm thresholds as `[[lo, hi]; 4]` (None if not a number).
    pub fn alarm_thresholds(&self) -> [[Option<f64>; 2]; NUM_PROBES] {
        let mut r = [[None; 2]; NUM_PROBES];
        for (i, a) in self.alarms.iter().enumerate() {
            r[i / 2][i % 2] = parse_num(&a.threshold);
        }
        r
    }

    pub fn json_num(v: Option<f64>) -> String {
        json_num(v)
    }
}

/// Mutable access to plugins from [`Core::with_ctx`].
pub struct PluginRefs<'a> {
    pub unkprobe: &'a mut UnkProbe,
    pub ramp: &'a mut Ramp,
}

static EMPTY: ConfigMap = BTreeMap::new();

/// Index used in `/set?al=` for a probe/kind (`L` = 0, `H` = 1).
pub fn alarm_index(probe: usize, kind: char) -> usize {
    probe * 2 + usize::from(kind == 'H')
}

/// Build the positional `al=` value that changes only one alarm.
pub fn alarm_set_value(probe: usize, kind: char, value: &str) -> String {
    format!("{}{}", ",".repeat(alarm_index(probe, kind)), value)
}

#[cfg(test)]
mod tests;
