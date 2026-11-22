//! Small least-squares helpers replacing `liblmfit-lua`.

/// Result of a fit.
#[derive(Debug, Clone, PartialEq)]
pub struct FitResult {
    pub params: Vec<f64>,
    /// Number of function evaluations / iterations used.
    pub evals: usize,
    /// lmfit-compatible status (1..=3 = converged).
    pub status: i32,
}

pub fn status_message(status: i32) -> &'static str {
    match status {
        1 => "success (sum of squares below underflow limit)",
        2 => "success (the relative error in the sum of squares is at most tol)",
        3 => "success (the relative error between x and the solution is at most tol)",
        4 => "trapped by degeneracy",
        5 => "timeout (number of calls to fcn has reached maxcall*(n+1))",
        _ => "failure (not enough data or singular system)",
    }
}

/// Ordinary least squares fit of `y = m*x + b`. Returns `[m, b]`.
pub fn linear(x: &[f64], y: &[f64]) -> Option<FitResult> {
    let n = x.len().min(y.len());
    if n < 2 {
        return None;
    }
    let nf = n as f64;
    let (sx, sy) = (x[..n].iter().sum::<f64>(), y[..n].iter().sum::<f64>());
    let (mx, my) = (sx / nf, sy / nf);
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for i in 0..n {
        let dx = x[i] - mx;
        sxx += dx * dx;
        sxy += dx * (y[i] - my);
    }
    if sxx == 0.0 {
        return None;
    }
    let m = sxy / sxx;
    Some(FitResult { params: vec![m, my - m * mx], evals: 1, status: 2 })
}

fn solve3(mut a: [[f64; 4]; 3]) -> Option<[f64; 3]> {
    // Gaussian elimination with partial pivoting on an augmented 3x4 matrix
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        for row in 0..3 {
            if row != col {
                let f = a[row][col] / a[col][col];
                let pivot = a[col];
                for (x, p) in a[row].iter_mut().zip(pivot).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    Some([a[0][3] / a[0][0], a[1][3] / a[1][1], a[2][3] / a[2][2]])
}

fn steinhart_temp(p: &[f64; 3], r: f64) -> f64 {
    let lx = r.ln();
    1.0 / ((p[2] * lx * lx + p[1]) * lx + p[0]) - 273.15
}

/// Fit Steinhart-Hart coefficients `1/T = A + B ln R + C (ln R)^3` from
/// resistance (ohms) and temperature (Celsius) pairs.
///
/// The model is linear in A, B, C in the 1/T domain, so a weighted linear
/// least squares solve gives the answer directly; a few Gauss-Newton steps
/// then minimise the error in the temperature domain like lmfit did.
pub fn steinhart(resists: &[f64], temps_c: &[f64]) -> Option<FitResult> {
    let n = resists.len().min(temps_c.len());
    if n < 3 || resists[..n].iter().any(|r| *r <= 0.0) {
        return None;
    }
    // Columns are scaled so the normal equations are well conditioned
    let rows: Vec<([f64; 3], f64)> = (0..n)
        .map(|i| {
            let l = resists[i].ln();
            ([1.0, l, l * l * l], 1.0 / (temps_c[i] + 273.15))
        })
        .collect();
    let scale = [1.0, 10.0, 1000.0];
    let normal = |rows: &[([f64; 3], f64)]| {
        let mut a = [[0.0; 4]; 3];
        for (x, y) in rows {
            let xs = [x[0] / scale[0], x[1] / scale[1], x[2] / scale[2]];
            for i in 0..3 {
                for j in 0..3 {
                    a[i][j] += xs[i] * xs[j];
                }
                a[i][3] += xs[i] * y;
            }
        }
        a
    };
    let s = solve3(normal(&rows))?;
    let mut p = [s[0] / scale[0], s[1] / scale[1], s[2] / scale[2]];

    // Refine in the temperature domain (Gauss-Newton with analytic Jacobian)
    let mut evals = 1;
    let sse = |p: &[f64; 3]| -> f64 { (0..n).map(|i| (steinhart_temp(p, resists[i]) - temps_c[i]).powi(2)).sum() };
    let mut cur = sse(&p);
    for _ in 0..20 {
        evals += 1;
        let mut a = [[0.0; 4]; 3];
        for i in 0..n {
            let l = resists[i].ln();
            let inv = (p[2] * l * l + p[1]) * l + p[0];
            let t = 1.0 / inv - 273.15;
            let d = -1.0 / (inv * inv);
            let j = [d / scale[0], d * l / scale[1], d * l * l * l / scale[2]];
            let r = temps_c[i] - t;
            for x in 0..3 {
                for y in 0..3 {
                    a[x][y] += j[x] * j[y];
                }
                a[x][3] += j[x] * r;
            }
        }
        let Some(dp) = solve3(a) else { break };
        let np = [p[0] + dp[0] / scale[0], p[1] + dp[1] / scale[1], p[2] + dp[2] / scale[2]];
        let ns = sse(&np);
        if !ns.is_finite() || ns >= cur {
            break;
        }
        let rel = (cur - ns) / cur.max(1e-300);
        p = np;
        cur = ns;
        if rel < 1e-12 {
            break;
        }
    }
    Some(FitResult { params: p.to_vec(), evals, status: 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_fit() {
        let x = [0.0, 1.0, 2.0, 3.0];
        let y = [1.0, 3.0, 5.0, 7.0];
        let r = linear(&x, &y).unwrap();
        assert!((r.params[0] - 2.0).abs() < 1e-12);
        assert!((r.params[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn steinhart_recovers_coefficients() {
        // Thermoworks preset
        let truth = [7.3431401e-4, 2.1574370e-4, 9.5156860e-8];
        let mut r = vec![];
        let mut t = vec![];
        for ohms in [3_000.0, 10_000.0, 30_000.0, 100_000.0, 300_000.0] {
            r.push(ohms);
            t.push(steinhart_temp(&truth, ohms));
        }
        let fit = steinhart(&r, &t).unwrap();
        for (i, v) in fit.params.iter().enumerate() {
            assert!((v - truth[i]).abs() / truth[i] < 1e-4, "param {i} {v} vs {}", truth[i]);
        }
    }
}
