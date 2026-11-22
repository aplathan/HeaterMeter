//! Number formatting compatible with Lua's `tostring` (`%.14g`).

/// Format a number like Lua 5.1 `tostring()` / `%.14g`.
pub fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "null".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let exp = v.abs().log10().floor() as i32;
    if !(-5..14).contains(&exp) {
        let s = format!("{:.13e}", v);
        let (mant, e) = s.split_once('e').unwrap_or((&s, "0"));
        let mant = trim_zeros(mant);
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{}e{}{:02}", mant, if e < 0 { '-' } else { '+' }, e.abs());
    }
    let decimals = (13 - exp).max(0) as usize;
    trim_zeros(&format!("{:.*}", decimals, v)).to_string()
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

/// Format an optional number as a JSON literal (`null` when absent).
pub fn json_num(v: Option<f64>) -> String {
    v.map(fmt_num).unwrap_or_else(|| "null".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_num(225.0), "225");
        assert_eq!(fmt_num(190.5), "190.5");
        assert_eq!(fmt_num(-40.0), "-40");
        assert_eq!(fmt_num(0.1 + 0.2), "0.3");
        assert_eq!(fmt_num(7.3431401e-4), "0.00073431401");
        assert_eq!(fmt_num(1.4440251e-9), "1.4440251e-09");
        assert_eq!(fmt_num(4.0), "4");
        assert_eq!(fmt_num(0.02), "0.02");
    }
}
