//! Peak detection and LidTrack (port of `linkmeter/peaks.lua`).

use crate::{Ctx, LogLevel};
use hm_protocol::{Command, StatusUpdate};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(u8)]
pub enum Mode {
    Unplug = 1,
    Startup = 2,
    Normal = 3,
    Lid = 4,
    Recover = 5,
}

impl Mode {
    fn name(m: Option<Mode>) -> &'static str {
        match m {
            Some(Mode::Unplug) => "UNPLUG",
            Some(Mode::Startup) => "STARTUP",
            Some(Mode::Normal) => "NORMAL",
            Some(Mode::Lid) => "LID",
            Some(Mode::Recover) => "RECOVER",
            None => "NONE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LidTrack {
    AwaitingLow,
    AwaitingHigh,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Peak {
    pub trend: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub val: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
}

impl Peak {
    fn describe(&self) -> String {
        format!(
            "@{}={:.1} half={} per={} amp={:.1} gain={:.1}",
            self.time.unwrap_or(0),
            self.val.unwrap_or(0.0),
            self.half.unwrap_or(0),
            self.period.unwrap_or(0),
            self.amp.unwrap_or(0.0),
            self.gain.unwrap_or(0.0)
        )
    }
}

/// Published peaks state (`peaks` SSE event).
#[derive(Debug, Clone, Serialize)]
pub struct PeaksState {
    #[serde(rename = "C")]
    pub c: Peak,
    #[serde(rename = "H")]
    pub h: Peak,
    #[serde(rename = "L")]
    pub l: Peak,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outhist: Option<Vec<u32>>,
    pub updatecnt: [u32; 5],
}

#[derive(Debug, Clone)]
pub struct Peaks {
    pub state: PeaksState,
    mode: Option<Mode>,
    setpoint: Option<String>,
    pit_log: Vec<f64>,
    lid_track: Option<LidTrack>,
    units_scale: f64,
    setpoint_scale: f64,
    trend_threshold: f64,
}

impl Default for Peaks {
    fn default() -> Self {
        Peaks {
            state: PeaksState {
                c: Peak { trend: 0, ..Default::default() },
                h: Peak { trend: 1, ..Default::default() },
                l: Peak { trend: -1, ..Default::default() },
                mode: None,
                outhist: None,
                updatecnt: [0; 5],
            },
            mode: None,
            setpoint: None,
            pit_log: Vec::new(),
            lid_track: None,
            units_scale: 1.0,
            setpoint_scale: 0.1,
            trend_threshold: 1.0,
        }
    }
}

impl Peaks {
    fn update_setpoint_scaling(&mut self, ctx: &Ctx) {
        let Some(sp) = self.setpoint.as_deref().and_then(hm_protocol::parse_num) else {
            self.units_scale = 1.0;
            self.setpoint_scale = 0.1;
            return;
        };
        let units = ctx.config_str("u").unwrap_or("F");
        let mut setpoint = sp;
        if units == "C" || units == "R" {
            self.units_scale = 5.0 / 9.0;
            setpoint = setpoint * 9.0 / 5.0 + 32.0;
        } else {
            self.units_scale = 1.0;
        }
        self.setpoint_scale = if setpoint < 250.0 {
            0.1
        } else if setpoint > 500.0 {
            0.4
        } else {
            0.3 * (setpoint - 250.0) / 250.0 + 0.1
        };
    }

    fn mode_change(&mut self, new: Mode, ctx: &mut Ctx) {
        ctx.log(LogLevel::Info, format!("PeakDetect mode change {}=>{}", Mode::name(self.mode), Mode::name(Some(new))));
        match new {
            Mode::Unplug => {
                *self = Peaks::default();
            }
            Mode::Startup => {
                self.state.outhist = None;
                self.pit_log.clear();
                self.update_setpoint_scaling(ctx);
            }
            Mode::Normal | Mode::Recover => {
                if self.lid_track.take().is_some() {
                    ctx.log(LogLevel::Info, "Lid mode ended normally, disabling LidTrack".into());
                }
            }
            Mode::Lid => self.lid_track = Some(LidTrack::AwaitingLow),
        }
        self.trend_threshold = if new == Mode::Lid {
            50.0 * self.units_scale * self.setpoint_scale
        } else {
            10.0 * self.units_scale * self.setpoint_scale
        };
        self.mode = Some(new);
        self.state.mode = Some(new as u8);
    }

    fn update_mode(&mut self, f: &[&str], ctx: &mut Ctx) {
        let mut new = self.mode;
        let mut force = false;
        let sp = f[0];
        if sp == "U" || f[1] == "U" {
            new = Some(Mode::Unplug);
        } else if Some(sp) != self.setpoint.as_deref() {
            new = Some(Mode::Startup);
            self.setpoint = Some(sp.to_string());
            force = true;
        } else if f.get(7).copied() != Some("0") {
            new = Some(Mode::Lid);
        } else if sp == "-"
            || matches!((hm_protocol::parse_num(sp), hm_protocol::parse_num(f[1])), (Some(s), Some(t)) if s <= t)
        {
            new = Some(Mode::Normal);
        } else if self.mode == Some(Mode::Lid) {
            new = Some(Mode::Recover);
        }
        if let Some(n) = new
            && (self.mode != Some(n) || force)
        {
            self.mode_change(n, ctx);
        }
    }

    fn peak_change(&mut self, new_trend: i32, ctx: &mut Ctx) {
        let cur = self.state.c.clone();
        let (half_ref, full_ref) = if cur.trend > 0 {
            (self.state.l.clone(), &mut self.state.h)
        } else {
            (self.state.h.clone(), &mut self.state.l)
        };
        let (mut half, mut amp, mut period, mut gain) = (None, None, None, None);
        if let (Some(ht), Some(hv)) = (half_ref.time, half_ref.val) {
            half = cur.time.map(|t| t - ht);
            amp = cur.val.map(|v| v - hv);
        }
        if let (Some(ft), Some(fv)) = (full_ref.time, full_ref.val) {
            period = cur.time.map(|t| t - ft);
            gain = cur.val.map(|v| v - fv);
        }
        full_ref.time = cur.time;
        full_ref.val = cur.val;
        full_ref.half = half;
        full_ref.period = period;
        full_ref.amp = amp;
        full_ref.gain = gain;
        let full_desc = full_ref.describe();
        let full_val = full_ref.val.unwrap_or(0.0);

        let last = self.pit_log.last().copied();
        self.pit_log = last.into_iter().chain(std::iter::once(full_val)).collect();

        let trend_name = match cur.trend {
            1 => "HIGH",
            -1 => "LOW",
            _ => "NONE",
        };
        ctx.log(LogLevel::Info, format!("New {trend_name} peak {full_desc}"));
        let ki = ctx.config_num("pidi");
        let kd = ctx.config_num("pidd");
        if let (Some(period), Some(0.0), Some(0.0)) = (period, ki, kd) {
            let ku = ctx.config_num("pidp").unwrap_or(1.0);
            let p = 0.6 * ku;
            let pf = period as f64;
            let (i, d) = (2.0 * p / pf, p * pf / 8.0 / 30.0);
            ctx.log(LogLevel::Info, format!("  Ziegler P={p:.1} I={i:.3} D={d:.1}"));
        }

        if self.lid_track == Some(LidTrack::AwaitingLow) && cur.trend == -1 {
            ctx.log(
                LogLevel::Info,
                format!(
                    "LidTrack detected low peak {:.1}, dropped {:.1}",
                    cur.val.unwrap_or(0.0),
                    self.state.h.val.unwrap_or(0.0) - cur.val.unwrap_or(0.0)
                ),
            );
            self.lid_track = Some(LidTrack::AwaitingHigh);
        }
        self.state.c.trend = new_trend;
    }

    fn detect(&mut self, now: i64, t: f64, ctx: &mut Ctx) {
        let mut new_trend = 0;
        for v in self.pit_log.iter().rev() {
            let diff = t - v;
            if diff > self.trend_threshold {
                new_trend = 1;
                break;
            }
            if diff < -self.trend_threshold {
                new_trend = -1;
                break;
            }
        }
        if self.state.h.time.is_none() {
            self.state.h.time = Some(now);
            self.state.h.val = Some(t);
        }
        if self.state.l.time.is_none() {
            self.state.l.time = Some(now);
            self.state.l.val = Some(t);
        }
        let c = &mut self.state.c;
        let set_cur = |c: &mut Peak| {
            c.time = Some(now);
            c.val = Some(t);
        };
        if c.trend == 0 {
            if new_trend != 0 {
                c.trend = new_trend;
                set_cur(c);
            }
        } else if c.trend > 0 {
            if t > c.val.unwrap_or(f64::MIN) {
                set_cur(c);
            } else if new_trend < 0 {
                self.peak_change(new_trend, ctx);
            }
        } else if t < c.val.unwrap_or(f64::MAX) {
            set_cur(c);
        } else if new_trend > 0 {
            self.peak_change(new_trend, ctx);
        }
    }

    fn update_pit_log(&mut self, now: i64, t: f64, ctx: &mut Ctx) {
        if now % 10 == 0 {
            self.pit_log.push(t);
            if self.pit_log.len() > 180 {
                self.pit_log.remove(0);
            }
            ctx.broadcast("peaks", serde_json::to_string(&self.state).unwrap_or_default());
        }
    }

    fn update_lid_track(&mut self, now: i64, t: f64, ctx: &mut Ctx) {
        if self.setpoint.as_deref() == Some("-") {
            return;
        }
        let mut dps = 0.0;
        let n = self.pit_log.len();
        if n > 1 {
            let phase = (now % 10) as f64;
            let t10 = (self.pit_log[n - 2] * (10.0 - phase) + self.pit_log[n - 1] * phase) / 10.0;
            dps = (t - t10) / 10.0;
        }
        let sp = self.setpoint.as_deref().and_then(hm_protocol::parse_num);
        if self.mode == Some(Mode::Normal) {
            if dps < -5.0 * self.units_scale * self.setpoint_scale && sp.is_some_and(|sp| t < sp) {
                ctx.log(LogLevel::Info, format!("LidTrack detected {dps:.2}deg/s drop, triggering Lid Mode"));
                if ctx.settings.lidtrack_enabled {
                    ctx.write(Command::set("ld", ",,1"));
                }
            }
        } else if self.lid_track == Some(LidTrack::AwaitingHigh) {
            let low = self.state.l.val.unwrap_or(t);
            if t - low > 20.0 * self.units_scale && dps > 0.0 && dps < 0.1 * self.units_scale {
                self.lid_track = None;
                ctx.log(
                    LogLevel::Info,
                    format!("LidTrack detected high plateau at {t:.1} ({dps:.2}deg/s), self-recovered {:.1}", t - low),
                );
                if ctx.settings.lidtrack_enabled {
                    ctx.write(Command::set("ld", ",,0"));
                }
            }
        }
    }

    pub fn on_status(&mut self, now: i64, st: &StatusUpdate, f: &[&str], ctx: &mut Ctx) {
        if f.len() < 8 {
            return;
        }
        self.update_mode(f, ctx);
        let Some(mode) = self.mode else { return };
        if mode == Mode::Unplug {
            return;
        }
        self.state.updatecnt[mode as usize - 1] += 1;
        let Some(pit) = st.temps[0] else { return };
        self.update_pit_log(now, pit, ctx);
        self.detect(now, pit, ctx);
        self.update_lid_track(now, pit, ctx);
        if mode == Mode::Normal
            && let Some(out) = st.output
        {
            let hist = self.state.outhist.get_or_insert_with(|| vec![0; 26]);
            let idx = ((out / 4.0).ceil() as i64).clamp(0, 25) as usize;
            hist[idx] += 1;
        }
    }

    /// Text dump (`$LMPS`).
    pub fn describe(&self) -> String {
        let s = &self.state;
        let join = |v: &[u32]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
        let mut f = vec![
            format!("Histogram={}", join(s.outhist.as_deref().unwrap_or(&[]))),
            format!("peaks.mode={} ({})", self.mode.map(|m| m as u8).unwrap_or(0), Mode::name(self.mode)),
            format!("peaks.updatecnt={}", join(&s.updatecnt)),
            format!(
                "peaksUnitsScale={:.2} peaksSetpointScale={:.2} peakTrendThreshold={:.2}",
                self.units_scale, self.setpoint_scale, self.trend_threshold
            ),
        ];
        if s.c.time.is_some() {
            f.push(format!("peaks.C ({:02}) {}", s.c.trend, s.c.describe()));
        }
        if s.h.time.is_some() {
            f.push(format!("peaks.H  {}", s.h.describe()));
        }
        if s.l.time.is_some() {
            f.push(format!("peaks.L  {}", s.l.describe()));
        }
        f.join("\n")
    }
}
