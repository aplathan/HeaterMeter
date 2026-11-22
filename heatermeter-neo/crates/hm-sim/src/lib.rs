//! A HeaterMeter firmware simulator.
//!
//! Implements the serial command set of `arduino/heatermeter/hmcore.cpp`
//! (`/config`, `/reboot`, `/set?...`) and produces status lines once per
//! simulated second using a crude thermal model of a smoker, so LinkMeter can
//! be developed and tested without hardware.

use hm_protocol::encode_line;

pub const VERSION: &str = "20210202B";

#[derive(Debug, Clone, Copy, PartialEq)]
enum PidMode {
    Off,
    Manual(f64),
    Auto,
}

#[derive(Debug, Clone)]
struct Probe {
    name: String,
    coeff: [String; 4],
    ptype: u8,
    offset: i32,
    alarm: [i32; 2],
    armed: [bool; 2],
    ringing: [bool; 2],
}

/// Simulated HeaterMeter.
#[derive(Debug, Clone)]
pub struct Simulator {
    probes: Vec<Probe>,
    /// Temperatures in Fahrenheit (index 0 = pit)
    temps: [f64; 4],
    plugged: [bool; 4],
    setpoint: f64,
    mode: PidMode,
    units: char,
    pid: [f64; 4], // b p i d
    fan: [i32; 8],
    lid_offset: i32,
    lid_duration: i32,
    lid_countdown: i32,
    lcd: [i32; 6],
    integral: f64,
    last_pit: f64,
    output: f64,
    output_avg: f64,
    report_pid: bool,
    rx: String,
    /// Ambient temperature (F)
    pub ambient: f64,
    /// Speed-up factor for the thermal model
    pub time_scale: f64,
    seconds: u64,
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

fn sci(v: f64) -> String {
    format!("{v:.7e}")
}

impl Simulator {
    pub fn new() -> Simulator {
        let names = ["Pit", "Food Probe1", "Food Probe2", "Ambient"];
        let probes = names
            .iter()
            .enumerate()
            .map(|(i, n)| Probe {
                name: n.to_string(),
                coeff: [sci(2.3067434e-4), sci(2.3696596e-4), sci(1.2636414e-7), sci(1.0e4)],
                ptype: if i < 3 { 1 } else { 0 },
                offset: 0,
                alarm: [-40, -200],
                armed: [false; 2],
                ringing: [false; 2],
            })
            .collect();
        Simulator {
            probes,
            temps: [70.0; 4],
            plugged: [true, true, true, false],
            setpoint: 225.0,
            mode: PidMode::Auto,
            units: 'F',
            pid: [0.0, 4.0, 0.02, 5.0],
            fan: [0, 100, 100, 200, 0, 100, 0, 100],
            lid_offset: 6,
            lid_duration: 240,
            lid_countdown: 0,
            lcd: [50, 255, 10, 2, 1, 127],
            integral: 0.0,
            last_pit: 70.0,
            output: 0.0,
            output_avg: 0.0,
            report_pid: false,
            rx: String::new(),
            ambient: 70.0,
            time_scale: 1.0,
            seconds: 0,
        }
    }

    /// Lines sent at power-on.
    pub fn boot(&self) -> String {
        format!("\n{}", encode_line(&format!("UCID,HeaterMeter,{VERSION}")))
    }

    fn to_units(&self, f: f64) -> f64 {
        if self.units == 'C' { (f - 32.0) * 5.0 / 9.0 } else { f }
    }

    fn units_to_f(&self, v: f64) -> f64 {
        if self.units == 'C' { v * 9.0 / 5.0 + 32.0 } else { v }
    }

    /// Set a probe temperature (F) and plug state, for tests.
    pub fn set_temp(&mut self, probe: usize, temp_f: f64, plugged: bool) {
        self.temps[probe] = temp_f;
        self.plugged[probe] = plugged;
    }

    /// Feed bytes received from the host; returns response bytes.
    pub fn receive(&mut self, data: &str) -> String {
        let mut out = String::new();
        for ch in data.chars() {
            if ch == '\n' || ch == '\r' {
                let line = std::mem::take(&mut self.rx);
                if line.starts_with('/') && line.len() <= hm_protocol::MAX_COMMAND_LEN {
                    out.push_str(&self.command(&line));
                }
            } else if self.rx.len() < 64 {
                self.rx.push(ch);
            } else {
                self.rx.clear();
            }
        }
        out
    }

    fn csv_i(v: &str, cur: &mut [i32]) {
        for (i, f) in v.split(',').enumerate() {
            if let (Some(c), Ok(n)) = (cur.get_mut(i), f.trim().parse::<f64>()) {
                *c = n as i32;
            }
        }
    }

    fn command(&mut self, line: &str) -> String {
        if line.starts_with("/config") {
            return self.config_dump();
        }
        if line.starts_with("/ucid") {
            return encode_line(&format!("UCID,HeaterMeter,{VERSION}"));
        }
        if line.starts_with("/reboo") {
            let mut fresh = self.clone();
            fresh.integral = 0.0;
            fresh.lid_countdown = 0;
            fresh.output = 0.0;
            *self = fresh;
            return self.boot();
        }
        let Some(rest) = line.strip_prefix("/set?") else { return String::new() };
        let Some((k, v)) = rest.split_once('=') else { return String::new() };
        match k {
            "sp" => {
                if let Some(u) = v.chars().last().filter(|c| c.is_ascii_alphabetic()) {
                    match u {
                        'F' | 'C' | 'A' | 'R' => self.units = u,
                        'O' => self.mode = PidMode::Off,
                        _ => {}
                    }
                }
                if v.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
                    let digits: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
                    if let Ok(n) = digits.parse::<i32>() {
                        if n > 0 {
                            self.setpoint = self.units_to_f(n as f64);
                            self.mode = PidMode::Auto;
                        } else {
                            self.mode = PidMode::Manual((-n) as f64);
                        }
                    }
                }
                self.pid_line()
            }
            k if k.starts_with("pid") => {
                let idx = match &k[3..] {
                    "b" => 0,
                    "p" => 1,
                    "i" => 2,
                    "d" => 3,
                    _ => return String::new(),
                };
                if let Ok(n) = v.parse() {
                    self.pid[idx] = n;
                }
                self.pid_line()
            }
            k if k.starts_with("pn") => {
                if let Some(p) = k[2..].parse::<usize>().ok().and_then(|i| self.probes.get_mut(i)) {
                    p.name = v.chars().take(12).collect();
                }
                self.names_line()
            }
            k if k.starts_with("pc") => {
                let Some(i) = k[2..].parse::<usize>().ok().filter(|i| *i < 4) else { return String::new() };
                for (n, f) in v.split(',').enumerate() {
                    if f.is_empty() {
                        continue;
                    }
                    match n {
                        0..=3 => {
                            if let Ok(x) = f.parse::<f64>() {
                                self.probes[i].coeff[n] = sci(x);
                            }
                        }
                        4 => {
                            if let Ok(t) = f.parse::<u32>() {
                                self.probes[i].ptype = if t >= 128 { 2 } else { t as u8 };
                                self.plugged[i] = self.probes[i].ptype != 0;
                            }
                        }
                        _ => {}
                    }
                }
                encode_line(&self.coeff_line(i))
            }
            "po" => {
                let mut o: Vec<i32> = self.probes.iter().map(|p| p.offset).collect();
                Self::csv_i(v, &mut o);
                for (p, v) in self.probes.iter_mut().zip(o) {
                    p.offset = v;
                }
                encode_line(&self.offsets_line())
            }
            "lb" => {
                Self::csv_i(v, &mut self.lcd);
                encode_line(&self.lcd_line())
            }
            "ld" => {
                let mut a = [self.lid_offset, self.lid_duration, -1];
                Self::csv_i(v, &mut a);
                self.lid_offset = a[0].max(0);
                self.lid_duration = a[1].max(30);
                match a[2] {
                    1 => self.lid_countdown = self.lid_duration,
                    0 => self.lid_countdown = 0,
                    _ => {}
                }
                encode_line(&self.lid_line())
            }
            "fn" => {
                Self::csv_i(v, &mut self.fan);
                encode_line(&self.fan_line())
            }
            "al" => {
                for (i, f) in v.split(',').enumerate().take(8) {
                    let Ok(n) = f.trim().parse::<i32>() else { continue };
                    let p = &mut self.probes[i / 2];
                    if n == 0 {
                        p.armed[i % 2] = false;
                        p.ringing[i % 2] = false;
                    } else {
                        p.alarm[i % 2] = n;
                        p.armed[i % 2] = n > 0;
                        p.ringing[i % 2] = false;
                    }
                }
                encode_line(&self.alarm_line())
            }
            "tp" => {
                let mut a = [i32::from(self.report_pid), -1];
                Self::csv_i(v, &mut a);
                self.report_pid = a[0] == 1;
                String::new()
            }
            _ => String::new(),
        }
    }

    fn pid_line(&self) -> String {
        encode_line(&format!("HMPD,0,{:.8},{:.8},{:.8},{}", self.pid[1], self.pid[2], self.pid[3], self.units))
    }

    fn names_line(&self) -> String {
        let n: Vec<&str> = self.probes.iter().map(|p| p.name.as_str()).collect();
        encode_line(&format!("HMPN,{}", n.join(",")))
    }

    fn coeff_line(&self, i: usize) -> String {
        let p = &self.probes[i];
        format!("HMPC,{i},{},{},{},{},{}", p.coeff[0], p.coeff[1], p.coeff[2], p.coeff[3], p.ptype)
    }

    fn offsets_line(&self) -> String {
        let o: Vec<String> = self.probes.iter().map(|p| p.offset.to_string()).collect();
        format!("HMPO,{}", o.join(","))
    }

    fn lcd_line(&self) -> String {
        let l: Vec<String> = self.lcd.iter().map(|v| v.to_string()).collect();
        format!("HMLB,{}", l.join(","))
    }

    fn lid_line(&self) -> String {
        format!("HMLD,{},{}", self.lid_offset, self.lid_duration)
    }

    fn fan_line(&self) -> String {
        let l: Vec<String> = self.fan.iter().map(|v| v.to_string()).collect();
        format!("HMFN,{}", l.join(","))
    }

    fn alarm_line(&self) -> String {
        let mut v = Vec::new();
        for p in &self.probes {
            for k in 0..2 {
                let val = if p.armed[k] { p.alarm[k] } else { -p.alarm[k].abs() };
                let suffix = if p.ringing[k] { if k == 0 { "L" } else { "H" } } else { "" };
                v.push(format!("{val}{suffix}"));
            }
        }
        format!("HMAL,{}", v.join(","))
    }

    fn config_dump(&self) -> String {
        let mut s = encode_line(&format!("UCID,HeaterMeter,{VERSION}"));
        s.push_str(&self.pid_line());
        s.push_str(&encode_line(&self.fan_line()));
        s.push_str(&self.names_line());
        for i in 0..4 {
            s.push_str(&encode_line(&self.coeff_line(i)));
        }
        s.push_str(&encode_line(&self.offsets_line()));
        s.push_str(&encode_line(&self.lid_line()));
        s.push_str(&encode_line(&self.lcd_line()));
        s.push_str(&encode_line(&self.alarm_line()));
        s.push_str(&encode_line("HMRM,,,,"));
        s
    }

    /// Advance one second and return the status output.
    pub fn tick(&mut self) -> String {
        self.seconds += 1;
        let dt = self.time_scale;
        let pit = self.temps[0];

        // PID
        self.output = match self.mode {
            PidMode::Off => 0.0,
            PidMode::Manual(o) => o,
            PidMode::Auto if self.lid_countdown > 0 => 0.0,
            PidMode::Auto => {
                let err = self.setpoint - pit;
                self.integral = (self.integral + self.pid[2] * err * dt).clamp(0.0, 100.0);
                let d = (pit - self.last_pit) / dt;
                (self.pid[0] + self.pid[1] * err + self.integral - self.pid[3] * d * 10.0).clamp(0.0, 100.0)
            }
        };
        self.output_avg = self.output_avg * 0.9 + self.output * 0.1;
        self.last_pit = pit;

        // Thermal model (F)
        let fan_heat = self.output * 0.0076;
        let loss = if self.lid_countdown > 0 && self.lid_countdown > self.lid_duration - 30 { 0.03 } else { 0.002 };
        self.temps[0] += (fan_heat - (pit - self.ambient) * loss) * dt;
        // Two pieces of meat of different size heat at different rates
        for (f, k) in [(1, 0.0004), (2, 0.00025)] {
            self.temps[f] += (self.temps[0] - self.temps[f]) * k * dt;
        }
        self.temps[3] = self.ambient;

        // Lid detection: temperature drop below setpoint - offset%
        if let PidMode::Auto = self.mode
            && self.lid_countdown > 0
        {
            self.lid_countdown -= dt as i32;
            if self.lid_countdown <= 0 || self.temps[0] >= self.setpoint {
                self.lid_countdown = 0;
            }
        }

        // Alarms
        let mut any_ringing = false;
        for i in 0..4 {
            let t = self.to_units(self.temps[i]);
            let p = &mut self.probes[i];
            if !self.plugged[i] {
                continue;
            }
            for k in 0..2 {
                let thr = p.alarm[k] as f64;
                if p.alarm[k] <= 0 {
                    continue;
                }
                let crossed = if k == 0 { t < thr } else { t > thr };
                if !crossed {
                    p.armed[k] = true;
                } else if p.armed[k] && !p.ringing[k] {
                    p.ringing[k] = true;
                }
                any_ringing |= p.ringing[k];
            }
        }

        let temp = |i: usize| -> String {
            if self.plugged[i] && self.probes[i].ptype != 0 {
                format!("{:.1}", self.to_units(self.temps[i]) + self.probes[i].offset as f64)
            } else {
                "U".into()
            }
        };
        let sp = match self.mode {
            PidMode::Off => "U".to_string(),
            PidMode::Manual(_) => "-".to_string(),
            PidMode::Auto => format!("{}", self.to_units(self.setpoint).round() as i64),
        };
        let fan_max = self.fan[1].max(1) as f64;
        let mut out = encode_line(&format!(
            "HMSU,{sp},{},{},{},{},{},{},{},{},{}",
            temp(0),
            temp(1),
            temp(2),
            temp(3),
            self.output.round() as i64,
            self.output_avg.round() as i64,
            self.lid_countdown.max(0),
            (self.output * fan_max / 100.0).round() as i64,
            self.output.round() as i64
        ));
        if any_ringing {
            out.push_str(&encode_line(&self.alarm_line()));
        }
        if self.seconds % 4 == 1 {
            out.push_str(&encode_line("HMAR,3,2,2,3,4,2"));
        }
        if self.report_pid {
            out.push_str(&encode_line(&format!(
                "HMPS,{:.2},{:.2},{:.2},{:.2},{:.2}",
                self.pid[0],
                self.pid[1] * (self.setpoint - pit),
                self.integral,
                0.0,
                self.temps[0] - pit
            )));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hm_protocol::{Segment, validate};

    fn lines(s: &str) -> Vec<String> {
        s.lines().filter(|l| !l.is_empty()).map(|l| validate(l).unwrap().line.to_string()).collect()
    }

    #[test]
    fn config_dump_complete() {
        let mut s = Simulator::new();
        let out = lines(&s.receive("\n/config\n"));
        let tags: Vec<String> = out.iter().map(|l| Segment::parse(l).tag.to_string()).collect();
        assert_eq!(
            tags,
            vec![
                "UCID", "HMPD", "HMFN", "HMPN", "HMPC", "HMPC", "HMPC", "HMPC", "HMPO", "HMLD", "HMLB", "HMAL", "HMRM"
            ]
        );
    }

    #[test]
    fn heats_up() {
        let mut s = Simulator::new();
        for _ in 0..1800 {
            s.tick();
        }
        assert!((s.temps[0] - 225.0).abs() < 15.0, "pit {}", s.temps[0]);
    }

    #[test]
    fn setpoint_and_units() {
        let mut s = Simulator::new();
        let out = s.receive("\n/set?sp=110C\n");
        assert!(out.contains("HMPD") && out.contains(",C*"));
        assert!((s.setpoint - 230.0).abs() < 0.01);
        s.receive("/set?sp=-40\n");
        assert_eq!(s.mode, PidMode::Manual(40.0));
    }

    #[test]
    fn alarm_rings_and_silences() {
        let mut s = Simulator::new();
        s.receive("/set?al=,,,100\n");
        s.set_temp(1, 150.0, true);
        // setting a threshold arms it, so an already crossed alarm rings
        let out = s.tick();
        assert!(out.contains("100H"), "{out}");
        let out = s.receive("/set?al=,,,0\n");
        assert!(!out.contains("100H"));
        // silenced alarm stays quiet until temperature goes back and crosses again
        assert!(!s.tick().contains("100H"));
        s.set_temp(1, 90.0, true);
        s.tick();
        s.set_temp(1, 150.0, true);
        assert!(s.tick().contains("100H"));
    }
}
