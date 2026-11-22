//! HeaterMeter serial protocol.
//!
//! HeaterMeter -> host lines look like `$TAG,f1,f2,...*HH\n` where `HH` is the
//! uppercase hex XOR of every byte between `$` and `*` (see
//! `arduino/heatermeter/serialxor.h`). The checksum is optional on the host side.
//!
//! Host -> HeaterMeter commands are plain `/command` lines (no checksum) that
//! must be at most 63 characters long including the leading `/`
//! (`g_SerialBuff[64]` in `hmcore.cpp`).

use std::fmt;

/// Maximum command length (including leading `/`) accepted by the AVR.
pub const MAX_COMMAND_LEN: usize = 63;
/// Number of temperature probes.
pub const NUM_PROBES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    /// Line did not start with `$`.
    NoDollar,
    /// Line had a `*HH` checksum that did not match.
    BadChecksum,
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineError::NoDollar => write!(f, "line does not start with '$'"),
            LineError::BadChecksum => write!(f, "checksum mismatch"),
        }
    }
}

impl std::error::Error for LineError {}

/// A validated line with the checksum removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated<'a> {
    /// The line without the `*HH` suffix, still starting with `$`.
    pub line: &'a str,
    /// Whether a checksum was present (and valid).
    pub checksummed: bool,
}

/// XOR checksum of a payload (bytes between `$` and `*`).
pub fn checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |acc, b| acc ^ b)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Validate a raw line (without trailing newline / CR).
///
/// Mirrors `segmentValidate` in the original `linkmeterd.lua`: a line must
/// start with `$`; a trailing `*HH` is verified if present, and a NUL byte in
/// the payload always fails the checksum.
pub fn validate(raw: &str) -> Result<Validated<'_>, LineError> {
    let raw = raw.trim_end_matches(['\r', '\n']);
    let bytes = raw.as_bytes();
    if bytes.first() != Some(&b'$') {
        return Err(LineError::NoDollar);
    }
    let n = bytes.len();
    if n >= 4
        && bytes[n - 3] == b'*'
        && let (Some(h), Some(l)) = (hex_val(bytes[n - 2]), hex_val(bytes[n - 1]))
    {
        let expected = (h << 4) | l;
        let payload = &bytes[1..n - 3];
        if payload.contains(&0) || checksum(payload) != expected {
            return Err(LineError::BadChecksum);
        }
        return Ok(Validated { line: &raw[..n - 3], checksummed: true });
    }
    Ok(Validated { line: raw, checksummed: false })
}

/// Encode a payload (without `$`) as a checksummed HeaterMeter line, including
/// the trailing newline. Used by the simulator and tests.
pub fn encode_line(payload: &str) -> String {
    format!("${}*{:02X}\n", payload, checksum(payload.as_bytes()))
}

/// A split segment: `$HMSU,1,2,3` -> tag `HMSU`, fields `["1","2","3"]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment<'a> {
    pub tag: &'a str,
    pub fields: Vec<&'a str>,
}

impl<'a> Segment<'a> {
    /// Split a validated line (with leading `$`, without checksum).
    pub fn parse(line: &'a str) -> Segment<'a> {
        let body = line.strip_prefix('$').unwrap_or(line);
        let mut it = body.split(',');
        let tag = it.next().unwrap_or("");
        // segSplit in Lua drops nothing but the tag; an absent field list yields []
        let fields = if body.contains(',') { it.collect() } else { Vec::new() };
        Segment { tag, fields }
    }

    pub fn field(&self, i: usize) -> Option<&'a str> {
        self.fields.get(i).copied()
    }
}

/// Parse a number the way Lua's `tonumber` treats HeaterMeter output:
/// returns `None` for `U`, `-`, empty strings and other non-numeric values.
pub fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Reject things Rust accepts but Lua would not produce from HM output
    if s.eq_ignore_ascii_case("inf") || s.eq_ignore_ascii_case("nan") || s.eq_ignore_ascii_case("infinity") {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `$HMSU` status update.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusUpdate {
    /// Raw setpoint field: number, `-` (manual) or `U` (off).
    pub setpoint: String,
    /// Probe temperatures, `None` when unplugged (`U`).
    pub temps: [Option<f64>; NUM_PROBES],
    pub output: Option<f64>,
    pub output_avg: Option<f64>,
    pub lid: Option<f64>,
    pub fan: Option<f64>,
    pub servo: Option<f64>,
}

impl StatusUpdate {
    /// Parse the field list of a `$HMSU` segment. Requires at least 8 fields
    /// (old firmware omits fan/servo).
    pub fn from_fields(f: &[&str]) -> Option<StatusUpdate> {
        if f.len() < 8 {
            return None;
        }
        let g = |i: usize| f.get(i).and_then(|v| parse_num(v));
        Some(StatusUpdate {
            setpoint: f[0].to_string(),
            temps: [g(1), g(2), g(3), g(4)],
            output: g(5),
            output_avg: g(6),
            lid: g(7),
            fan: g(8),
            servo: g(9),
        })
    }

    pub fn setpoint_num(&self) -> Option<f64> {
        parse_num(&self.setpoint)
    }

    pub fn is_manual(&self) -> bool {
        self.setpoint == "-"
    }
}

/// One alarm entry of `$HMAL`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmValue {
    /// Threshold as sent (without ringing suffix), e.g. `"-200"`.
    pub raw: String,
    pub ringing: bool,
}

pub fn parse_alarm_field(v: &str) -> AlarmValue {
    match v.as_bytes().last() {
        Some(b'H') | Some(b'L') => AlarmValue { raw: v[..v.len() - 1].to_string(), ringing: true },
        _ => AlarmValue { raw: v.to_string(), ringing: false },
    }
}

/// RF node info from `$HMRF`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfNode {
    pub id: String,
    pub lobatt: bool,
    pub reset: bool,
    pub native: bool,
    pub rssi: i32,
}

pub fn parse_rf_update(fields: &[&str]) -> Vec<RfNode> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < fields.len() {
        let flags: u32 = fields[i + 1].trim().parse().unwrap_or(0);
        out.push(RfNode {
            id: fields[i].to_string(),
            lobatt: flags & 0x01 != 0,
            reset: flags & 0x02 != 0,
            native: flags & 0x04 != 0,
            rssi: fields.get(i + 2).and_then(|v| v.trim().parse().ok()).unwrap_or(0),
        });
        i += 3;
    }
    out
}

/// Host-interactive menu buttons (`menus.h`).
pub mod button {
    pub const LEFT: u8 = 0x01;
    pub const RIGHT: u8 = 0x02;
    pub const UP: u8 = 0x04;
    pub const DOWN: u8 = 0x08;
    pub const TIMEOUT: u8 = 0x20;
    pub const LEAVE: u8 = 0x40;
    pub const ENTER: u8 = 0x80;
}

/// Commands sent to HeaterMeter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `/config`: dump all configuration.
    Config,
    /// `/reboot`
    Reboot,
    /// `/ucid`
    Ucid,
    /// `/set?key=value`
    Set { key: String, value: String },
}

impl Command {
    pub fn set(key: impl Into<String>, value: impl Into<String>) -> Command {
        Command::Set { key: key.into(), value: value.into() }
    }

    /// The command text without framing, e.g. `/set?sp=225`.
    pub fn text(&self) -> String {
        match self {
            Command::Config => "/config".into(),
            Command::Reboot => "/reboot".into(),
            Command::Ucid => "/ucid".into(),
            Command::Set { key, value } => format!("/set?{key}={value}"),
        }
    }

    /// Wire bytes as written by linkmeterd: a leading `\n` flushes any partial
    /// line on the AVR side.
    pub fn wire(&self) -> String {
        format!("\n{}\n", self.text())
    }

    /// Check the command fits the AVR line buffer.
    pub fn fits(&self) -> bool {
        self.text().len() <= MAX_COMMAND_LEN
    }
}

/// Keys accepted after `/set?` (used to validate API input).
pub fn is_valid_set_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 8 && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Normalize a setpoint value the way `lmadmin.api_set` did: uppercase,
/// `NN%` -> `-NN`, `OFF` -> `O`.
pub fn normalize_setpoint(v: &str) -> String {
    let v = v.trim().to_ascii_uppercase();
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    if v.ends_with('%') {
        return format!("-{digits}");
    }
    if v.ends_with("OFF") {
        return "O".into();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_roundtrip() {
        let l = encode_line("UCID,HeaterMeter,20210202B");
        assert!(l.starts_with("$UCID"));
        let v = validate(l.trim_end()).unwrap();
        assert!(v.checksummed);
        assert_eq!(v.line, "$UCID,HeaterMeter,20210202B");
    }

    #[test]
    fn checksum_bad() {
        assert_eq!(validate("$HMSU,225,U,U,U,U,0,0,0,0,0*00"), Err(LineError::BadChecksum));
    }

    #[test]
    fn no_checksum_ok() {
        let v = validate("$HMSU,225,U,U,U,U,0,0,0").unwrap();
        assert!(!v.checksummed);
    }

    #[test]
    fn no_dollar() {
        assert_eq!(validate("garbage"), Err(LineError::NoDollar));
        assert_eq!(validate(""), Err(LineError::NoDollar));
    }

    #[test]
    fn nul_fails() {
        let payload = "HMLG,a\0b";
        let line = format!("${}*{:02X}", payload, checksum(payload.as_bytes()));
        assert_eq!(validate(&line), Err(LineError::BadChecksum));
    }

    #[test]
    fn crlf_stripped() {
        let l = encode_line("HMPO,0,0,0,0").replace('\n', "\r\n");
        assert_eq!(validate(&l).unwrap().line, "$HMPO,0,0,0,0");
    }

    #[test]
    fn segment_split() {
        let s = Segment::parse("$HMSU,225,190.5,U,U,U,50,40,0,50,0");
        assert_eq!(s.tag, "HMSU");
        assert_eq!(s.fields.len(), 10);
        let st = StatusUpdate::from_fields(&s.fields).unwrap();
        assert_eq!(st.temps, [Some(190.5), None, None, None]);
        assert_eq!(st.fan, Some(50.0));
        let s = Segment::parse("$HMRC");
        assert!(s.fields.is_empty());
        let s = Segment::parse("$HMRM,,,,");
        assert_eq!(s.fields, vec!["", "", "", ""]);
    }

    #[test]
    fn status_short() {
        let s = Segment::parse("$HMSU,-,200,U,U,U,30,25,0");
        let st = StatusUpdate::from_fields(&s.fields).unwrap();
        assert!(st.is_manual());
        assert_eq!(st.fan, None);
        assert!(StatusUpdate::from_fields(&["1", "2"]).is_none());
    }

    #[test]
    fn alarms() {
        let a = parse_alarm_field("200H");
        assert!(a.ringing);
        assert_eq!(a.raw, "200");
        let a = parse_alarm_field("-40");
        assert!(!a.ringing);
    }

    #[test]
    fn rf() {
        let n = parse_rf_update(&["255", "0", "12", "3", "5", "-60"]);
        assert_eq!(n.len(), 2);
        assert!(n[1].lobatt && !n[1].reset && n[1].native);
        assert_eq!(n[1].rssi, -60);
    }

    #[test]
    fn commands() {
        assert_eq!(Command::set("sp", "225F").wire(), "\n/set?sp=225F\n");
        assert!(Command::set("al", "0,0,0,0,0,0,0,0").fits());
        assert!(!Command::set("tt", "x".repeat(80)).fits());
    }

    #[test]
    fn setpoint_normalize() {
        assert_eq!(normalize_setpoint("50%"), "-50");
        assert_eq!(normalize_setpoint("off"), "O");
        assert_eq!(normalize_setpoint("225f"), "225F");
        assert_eq!(normalize_setpoint("0r"), "0R");
    }
}
