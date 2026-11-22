//! Smaller plugins: ramp, degrees-per-hour, unknown probe calibration and the
//! LCD network info menu. Ports of `linkmeter/{ramp,dph,unkprobe,ipwatch}.lua`.

use crate::{Ctx, Effect, LogLevel, fit, num::fmt_num};
use hm_protocol::{Command, NUM_PROBES, StatusUpdate, button};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

// ---------------------------------------------------------------- ramp

/// Ramp configuration (persisted by the daemon).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RampParams {
    pub enabled: bool,
    /// Probe indices to watch (averaged)
    pub watch: Vec<usize>,
    pub trigger: f64,
    pub target: f64,
    /// Setpoint at the start of an active ramp (survives power loss)
    pub start_setpoint: Option<f64>,
}

impl Default for RampParams {
    fn default() -> Self {
        RampParams { enabled: false, watch: vec![1], trigger: 180.0, target: 205.0, start_setpoint: None }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Ramp {
    pub params: RampParams,
    ramping: bool,
    update_cnt: i32,
    start_setpoint: Option<f64>,
    last_setpoint: Option<f64>,
    requested_setpoint: Option<f64>,
}

impl Ramp {
    pub fn new(params: RampParams) -> Ramp {
        let mut r = Ramp::default();
        r.reload(params);
        r
    }

    pub fn reload(&mut self, params: RampParams) {
        self.start_setpoint = if params.enabled { params.start_setpoint } else { None };
        self.params = params;
    }

    fn cancel(&mut self, ctx: &mut Ctx) {
        self.params.enabled = false;
        self.params.start_setpoint = None;
        self.start_setpoint = None;
        self.ramping = false;
        ctx.effects.push(Effect::RampCancelled);
        ctx.publish_global("ramp", None);
    }

    pub fn on_status(&mut self, st: &StatusUpdate, ctx: &mut Ctx) {
        let setpoint = st.setpoint_num();
        if self.last_setpoint != setpoint {
            if self.ramping && setpoint != self.requested_setpoint {
                ctx.log(LogLevel::Warning, "Unrequested setpoint change detected. Ramp canceled.".into());
                self.cancel(ctx);
            }
            self.last_setpoint = setpoint;
        }
        let Some(setpoint) = setpoint else {
            if self.ramping {
                self.cancel(ctx);
            }
            return;
        };
        if !self.params.enabled || self.params.watch.is_empty() {
            if self.ramping {
                self.cancel(ctx);
            }
            return;
        }
        let vals: Vec<f64> = self.params.watch.iter().filter_map(|w| st.temps.get(*w).copied().flatten()).collect();
        if vals.is_empty() {
            return;
        }
        let watch = vals.iter().sum::<f64>() / self.params.watch.len() as f64;
        let (trigger, target) = (self.params.trigger, self.params.target);

        if !self.ramping && (watch > trigger || self.start_setpoint.is_some()) {
            self.ramping = true;
            self.update_cnt = 0;
            if self.start_setpoint.is_none() {
                self.start_setpoint = Some(setpoint);
                self.params.start_setpoint = Some(setpoint);
                ctx.effects.push(Effect::RampStarted { start_setpoint: setpoint });
            }
            ctx.log(
                LogLevel::Warning,
                format!(
                    "Beginning ramp: Set({}->{}) Watch({}->{})",
                    fmt_num(self.start_setpoint.unwrap_or(setpoint)),
                    fmt_num(target),
                    fmt_num(trigger),
                    fmt_num(target)
                ),
            );
        }
        if !self.ramping {
            return;
        }
        let mut new_sp = setpoint;
        if target - watch < 1.0 || target == trigger {
            ctx.log(LogLevel::Warning, "Ramp complete".into());
            new_sp = target;
            self.cancel(ctx);
        } else {
            self.update_cnt -= 1;
            if self.update_cnt <= 0 {
                self.update_cnt = 60;
                let start = self.start_setpoint.unwrap_or(setpoint);
                let progress = (target - watch) / (target - trigger);
                new_sp = ((start - target) * progress).ceil() + target;
                ctx.publish_global(
                    "ramp",
                    Some(format!(
                        "{{\"s\":{},\"ta\":{},\"tr\":{}}}",
                        fmt_num(start),
                        fmt_num(target),
                        fmt_num(trigger)
                    )),
                );
            }
        }
        if new_sp != setpoint {
            self.requested_setpoint = Some(new_sp);
            ctx.write(Command::set("sp", fmt_num(new_sp)));
            ctx.log(
                LogLevel::Warning,
                format!("New ramp setpoint {} vs {} at watchVal {:.1}", fmt_num(new_sp), fmt_num(setpoint), watch),
            );
        }
    }
}

// ---------------------------------------------------------------- dph

/// Degrees-per-hour estimate from a linear fit over the last hour.
#[derive(Debug, Default, Clone)]
pub struct Dph {
    samples: VecDeque<(i64, [Option<f64>; NUM_PROBES])>,
    last_update: i64,
}

impl Dph {
    const WINDOW: i64 = 3600;
    const STEP: i64 = 10;

    pub fn on_status(&mut self, now: i64, st: &StatusUpdate, ctx: &mut Ctx) {
        self.samples.push_back((now, st.temps));
        while self.samples.front().is_some_and(|(t, _)| *t < now - Self::WINDOW) {
            self.samples.pop_front();
        }
        if now - self.last_update < 60 {
            return;
        }
        self.last_update = now;
        let start = now - Self::WINDOW;
        let buckets = (Self::WINDOW / Self::STEP) as usize;
        for probe in 0..NUM_PROBES {
            let mut sum = vec![0.0; buckets];
            let mut cnt = vec![0u32; buckets];
            for (t, temps) in &self.samples {
                if let Some(v) = temps[probe] {
                    let b = ((t - start) / Self::STEP).clamp(0, buckets as i64 - 1) as usize;
                    sum[b] += v;
                    cnt[b] += 1;
                }
            }
            let (mut x, mut y) = (Vec::new(), Vec::new());
            for b in 0..buckets {
                if cnt[b] > 0 {
                    x.push((b + 1) as f64 / 360.0);
                    y.push(sum[b] / cnt[b] as f64);
                }
            }
            let dph =
                if x.len() > 180 { fit::linear(&x, &y).map(|r| (r.params[0] * 100.0).round() / 100.0) } else { None };
            ctx.publish_probe(probe, "dph", dph.map(fmt_num));
        }
    }
}

// ---------------------------------------------------------------- unkprobe

/// Unknown thermistor calibration: probe 0 is a known reference, probe 1 is
/// reported as resistance (`sp=0R`).
#[derive(Debug, Default, Clone)]
pub struct UnkProbe {
    /// temperature (x10, as key) -> resistance
    points: Option<BTreeMap<i64, f64>>,
    discard: Option<i32>,
}

impl UnkProbe {
    pub fn active(&self) -> bool {
        self.points.is_some()
    }

    pub fn start(&mut self, ctx: &mut Ctx) {
        self.points = Some(BTreeMap::new());
        self.discard = Some(1);
        ctx.write(Command::set("sp", "0R"));
    }

    pub fn stop(&mut self, ctx: &mut Ctx) -> bool {
        if self.points.take().is_some() {
            ctx.write(Command::Reboot);
            true
        } else {
            false
        }
    }

    pub fn on_status(&mut self, st: &StatusUpdate) {
        let Some(points) = self.points.as_mut() else { return };
        let (Some(t), Some(r)) = (st.temps[0], st.temps[1]) else { return };
        if let Some(d) = self.discard.as_mut() {
            *d -= 1;
            if *d >= 0 {
                return;
            }
            self.discard = None;
        }
        points.insert((t * 10.0).round() as i64, r.floor());
    }

    pub fn points(&self) -> Option<Vec<(f64, f64)>> {
        self.points.as_ref().map(|p| p.iter().map(|(k, v)| (*k as f64 / 10.0, *v)).collect())
    }

    pub fn csv(&self) -> Option<String> {
        let pts = self.points()?;
        let mut r = vec!["C,R".to_string()];
        r.extend(pts.iter().map(|(t, ohm)| format!("{t:.1},{ohm}")));
        Some(r.join("\n"))
    }

    pub fn fit_json(&self) -> Option<String> {
        let pts = self.points()?;
        let (t, r): (Vec<f64>, Vec<f64>) = pts.into_iter().unzip();
        Some(match fit::steinhart(&r, &t) {
            Some(f) => format!(
                "{{\"a\":{:.7e},\"b\":{:.7e},\"c\":{:.7e},\"n\":{},\"e\":{},\"status\":{},\"message\":\"{}\"}}",
                f.params[0],
                f.params[1],
                f.params[2],
                t.len(),
                f.evals,
                f.status,
                fit::status_message(f.status)
            ),
            None => format!("{{\"n\":{},\"e\":0,\"status\":0,\"message\":\"{}\"}}", t.len(), fit::status_message(0)),
        })
    }
}

// ---------------------------------------------------------------- netinfo

/// LCD "Network Information" host-interactive menu (topic 0).
///
/// HeaterMeter forwards button presses on its NETINFO menu page to the host
/// (`$HMHI`), so the board's own buttons can drive these pages:
/// title, IP address, wifi status and a wifi access point toggle (DOWN on the
/// last page starts/stops the AP).
#[derive(Debug, Default, Clone)]
pub struct NetInfo {
    pub ip: Option<String>,
    /// Short wifi status line, e.g. the SSID or `AP heatermeter`
    pub wifi: Option<String>,
    pub ap_active: bool,
}

pub const TOPIC_NETINFO: u8 = 0;

impl NetInfo {
    pub fn on_interactive(&mut self, topic: u8, opaque: u8, btn: u8, ctx: &mut Ctx) {
        if topic != TOPIC_NETINFO {
            return;
        }
        const TITLE: u8 = 0;
        const LAST: u8 = 3;
        let mut opaque = opaque.min(LAST);
        let mut toggled = false;
        if btn == button::DOWN && opaque == LAST {
            // Already on the AP page: DOWN toggles the access point
            toggled = true;
            ctx.effects.push(Effect::WifiAp { start: !self.ap_active });
        } else if btn == button::DOWN && opaque < LAST {
            opaque += 1;
        } else if btn == button::UP && opaque > TITLE {
            opaque -= 1;
        }
        let (l1, l2) = match opaque {
            0 => ("\u{2}   Network   \u{2}".to_string(), "  Information  ".to_string()),
            1 => ("Network Address".to_string(), self.ip.clone().unwrap_or_else(|| "Unknown".into())),
            2 => ("Wifi".to_string(), self.wifi.clone().unwrap_or_else(|| "Not connected".into())),
            _ if toggled && !self.ap_active => ("Starting AP...".to_string(), "192.168.201.1".to_string()),
            _ if toggled => ("Stopping AP...".to_string(), "Joining wifi".to_string()),
            _ if self.ap_active => ("Stop Wifi AP".to_string(), "Press DOWN".to_string()),
            _ => ("Start Wifi AP".to_string(), "Press DOWN".to_string()),
        };
        let clip = |s: String| s.replace(',', " ").chars().take(16).collect::<String>();
        ctx.write(Command::set("hi", format!("{opaque},{},{}", clip(l1), clip(l2))));
    }
}
