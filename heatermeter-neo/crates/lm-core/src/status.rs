//! Status JSON (the `hmstatus` document) and update throttling.
//!
//! The JSON layout is byte-compatible with the `JSON_TEMPLATE` built by the
//! original `linkmeterd.lua`, which HeaterMeterClient (MeterMonitor, SMUSHI)
//! and third party apps parse.

use crate::num::fmt_num;
use hm_protocol::NUM_PROBES;

pub const DEFAULT_NAMES: [&str; NUM_PROBES] = ["Pit", "Food Probe1", "Food Probe2", "Ambient"];

/// Ordered key -> JSON literal list (keeps insertion order).
#[derive(Debug, Clone, Default)]
pub struct ExtVals(Vec<(String, String)>);

impl ExtVals {
    /// Set or remove (`None`) a value. Returns true if anything changed.
    pub fn set(&mut self, key: &str, json: Option<String>) -> bool {
        let pos = self.0.iter().position(|(k, _)| k == key);
        match (pos, json) {
            (Some(i), Some(v)) => {
                if self.0[i].1 == v {
                    false
                } else {
                    self.0[i].1 = v;
                    true
                }
            }
            (Some(i), None) => {
                self.0.remove(i);
                true
            }
            (None, Some(v)) => {
                self.0.push((key.to_string(), v));
                true
            }
            (None, None) => false,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn write(&self, out: &mut String) {
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&serde_json::to_string(k).unwrap_or_default());
            out.push(':');
            out.push_str(v);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ProbeStatus {
    pub name: String,
    /// Current temperature JSON literal (`null` if unplugged)
    pub temp: String,
    pub ext: ExtVals,
    /// Alarm low/high thresholds as JSON literals
    pub alarm_lo: String,
    pub alarm_hi: String,
    /// Ringing alarm: `Some('L')`, `Some('H')` or `None`
    pub ringing: Option<char>,
}

/// Current status, rendered on demand to the legacy JSON format.
#[derive(Debug, Clone)]
pub struct StatusState {
    pub time: i64,
    pub set: String,
    pub lid: String,
    pub fan_c: String,
    pub fan_a: String,
    pub fan_f: String,
    pub fan_s: String,
    pub ext: ExtVals,
    pub probes: [ProbeStatus; NUM_PROBES],
}

impl Default for StatusState {
    fn default() -> Self {
        let probe = |i: usize| ProbeStatus {
            name: DEFAULT_NAMES[i].to_string(),
            temp: "0".into(),
            ext: ExtVals::default(),
            alarm_lo: "null".into(),
            alarm_hi: "null".into(),
            ringing: None,
        };
        StatusState {
            time: 0,
            set: "0".into(),
            lid: "0".into(),
            fan_c: "0".into(),
            fan_a: "0".into(),
            fan_f: "null".into(),
            fan_s: "null".into(),
            ext: ExtVals::default(),
            probes: [probe(0), probe(1), probe(2), probe(3)],
        }
    }
}

/// JSON literal for a raw HeaterMeter field: number or `null`.
pub fn lit(raw: &str) -> String {
    hm_protocol::parse_num(raw).map(fmt_num).unwrap_or_else(|| "null".into())
}

impl StatusState {
    /// Apply a `$HMSU` field list (with the manual-mode setpoint already
    /// rewritten to `-<output>`).
    pub fn apply(&mut self, time: i64, f: &[&str]) {
        let g = |i: usize| f.get(i).map(|v| lit(v)).unwrap_or_else(|| "null".into());
        self.time = time;
        self.set = g(0);
        for (i, p) in self.probes.iter_mut().enumerate() {
            p.temp = g(1 + i);
        }
        self.fan_c = g(5);
        self.fan_a = g(6);
        self.lid = g(7);
        self.fan_f = g(8);
        self.fan_s = g(9);
    }

    /// Current temperature of a probe, if any.
    pub fn temp(&self, probe: usize) -> Option<f64> {
        self.probes.get(probe).and_then(|p| p.temp.parse().ok())
    }

    pub fn has_update(&self) -> bool {
        self.time != 0
    }

    /// Render the status JSON. Returns `{}` before the first status update.
    pub fn to_json(&self) -> String {
        if !self.has_update() {
            return "{}".into();
        }
        let mut s = String::with_capacity(512);
        s.push_str("{\"time\":");
        s.push_str(&self.time.to_string());
        s.push_str(",\"set\":");
        s.push_str(&self.set);
        s.push_str(",\"lid\":");
        s.push_str(&self.lid);
        s.push_str(",\"fan\":{\"c\":");
        s.push_str(&self.fan_c);
        s.push_str(",\"a\":");
        s.push_str(&self.fan_a);
        s.push_str(",\"f\":");
        s.push_str(&self.fan_f);
        s.push_str(",\"s\":");
        s.push_str(&self.fan_s);
        s.push_str("},");
        if !self.ext.is_empty() {
            self.ext.write(&mut s);
            s.push(',');
        }
        s.push_str("\"temps\":[");
        for (i, p) in self.probes.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"n\":");
            s.push_str(&serde_json::to_string(&p.name).unwrap_or_default());
            s.push_str(",\"c\":");
            s.push_str(&p.temp);
            if !p.ext.is_empty() {
                s.push(',');
                p.ext.write(&mut s);
            }
            s.push_str(",\"a\":{\"l\":");
            s.push_str(&p.alarm_lo);
            s.push_str(",\"h\":");
            s.push_str(&p.alarm_hi);
            s.push_str(",\"r\":");
            match p.ringing {
                Some(c) => {
                    s.push('"');
                    s.push(c);
                    s.push('"');
                }
                None => s.push_str("null"),
            }
            s.push_str("}}");
        }
        s.push_str("]}");
        s
    }
}

/// Port of `throttleUpdate()`:
/// SLOW: if the line is unchanged only every fifth update is sent;
/// NORMAL: if the line changed only every second update;
/// a changed line during a SLOW period is never skipped.
#[derive(Debug, Clone)]
pub struct Throttle {
    last: Option<String>,
    spare: u32,
    skipped: u32,
}

impl Default for Throttle {
    fn default() -> Self {
        let mut t = Throttle { last: None, spare: 0, skipped: 0 };
        t.unthrottle();
        t
    }
}

impl Throttle {
    const MAX_SEQUENTIAL: u32 = 2;
    const MAX_THROTTLED: u32 = 4;

    /// Force the next two updates through.
    pub fn unthrottle(&mut self) {
        self.skipped = 99;
        self.spare = 2;
    }

    /// Returns true if the update should be skipped.
    pub fn should_skip(&mut self, line: &str) -> bool {
        if self.last.as_deref() == Some(line) {
            if self.skipped >= 2 && self.spare < Self::MAX_SEQUENTIAL - 1 {
                self.spare += 1;
            }
            if self.skipped < Self::MAX_THROTTLED {
                self.skipped += 1;
                return true;
            }
        } else if self.skipped == 0 {
            if self.spare == 0 {
                self.skipped += 1;
                return true;
            }
            self.spare -= 1;
        }
        self.last = Some(line.to_string());
        self.skipped = 0;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_sequence() {
        // Hand-traced output of the original Lua throttleUpdate() (its comment's
        // example sequence is not what the code actually does).
        let mut t = Throttle { last: None, spare: 0, skipped: 1 }; // steady state after a sent line
        let input = ["A", "B", "C", "D", "E", "E", "E", "E", "F", "G", "H"];
        let out: Vec<&str> = input.iter().filter(|l| !t.should_skip(l)).copied().collect();
        assert_eq!(out, vec!["A", "C", "E", "F", "G"]);
    }

    #[test]
    fn unthrottle_passes_two() {
        let mut t = Throttle::default();
        assert!(!t.should_skip("A"));
        assert!(!t.should_skip("B"));
        assert!(!t.should_skip("C"));
        assert!(t.should_skip("D"));
    }

    #[test]
    fn empty_status() {
        assert_eq!(StatusState::default().to_json(), "{}");
    }

    #[test]
    fn json_layout() {
        let mut s = StatusState::default();
        s.apply(1700000000, &["225", "190.5", "U", "U", "U", "50", "40", "0", "50", "0"]);
        s.probes[1].ext.set("dph", Some("1.5".into()));
        s.ext.set("adc", Some("[1,2]".into()));
        assert_eq!(
            s.to_json(),
            "{\"time\":1700000000,\"set\":225,\"lid\":0,\"fan\":{\"c\":50,\"a\":40,\"f\":50,\"s\":0},\"adc\":[1,2],\"temps\":[{\"n\":\"Pit\",\"c\":190.5,\"a\":{\"l\":null,\"h\":null,\"r\":null}},{\"n\":\"Food Probe1\",\"c\":null,\"dph\":1.5,\"a\":{\"l\":null,\"h\":null,\"r\":null}},{\"n\":\"Food Probe2\",\"c\":null,\"a\":{\"l\":null,\"h\":null,\"r\":null}},{\"n\":\"Ambient\",\"c\":null,\"a\":{\"l\":null,\"h\":null,\"r\":null}}]}"
        );
        let v: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(v["temps"][0]["c"], 190.5);
    }
}
