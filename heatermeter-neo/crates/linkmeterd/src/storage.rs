//! History storage in SQLite, replacing rrdtool.
//!
//! Samples (1 Hz) are grouped into *sessions* (cooks). Writes are buffered
//! and flushed in a single transaction every few seconds to spare the SD
//! card. Downsampling happens at query time.

use lm_core::Sample;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionInfo {
    pub id: i64,
    pub name: String,
    pub started: i64,
    pub last: i64,
    pub samples: i64,
    pub current: bool,
}

/// Bucketed history, column oriented (ready for uPlot).
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct History {
    pub step: i64,
    pub time: Vec<i64>,
    pub sp: Vec<Option<f64>>,
    pub t0: Vec<Option<f64>>,
    pub t1: Vec<Option<f64>>,
    pub t2: Vec<Option<f64>>,
    pub t3: Vec<Option<f64>>,
    pub out: Vec<Option<f64>>,
    pub fan: Vec<Option<f64>>,
    pub servo: Vec<Option<f64>>,
}

impl History {
    fn push_row(&mut self, t: i64, v: [Option<f64>; 8]) {
        self.time.push(t);
        self.sp.push(v[0]);
        self.t0.push(v[1]);
        self.t1.push(v[2]);
        self.t2.push(v[3]);
        self.t3.push(v[4]);
        self.out.push(v[5]);
        self.fan.push(v[6]);
        self.servo.push(v[7]);
    }

    pub fn len(&self) -> usize {
        self.time.len()
    }

    fn row(&self, i: usize) -> [Option<f64>; 8] {
        [self.sp[i], self.t0[i], self.t1[i], self.t2[i], self.t3[i], self.out[i], self.fan[i], self.servo[i]]
    }

    /// CSV in the legacy `lm/hist` format: `time,sp,t0,t1,t2,t3,out,fan,servo`.
    pub fn to_csv(&self, header: bool) -> String {
        let mut s = String::new();
        if header {
            s.push_str("time,setpoint,probe0,probe1,probe2,probe3,output,fan,servo\n");
        }
        for i in 0..self.len() {
            s.push_str(&self.time[i].to_string());
            for v in self.row(i) {
                s.push(',');
                match v {
                    Some(v) => s.push_str(&lm_core::num::fmt_num((v * 1000.0).round() / 1000.0)),
                    None => s.push_str("nan"),
                }
            }
            s.push('\n');
        }
        s
    }
}

struct Inner {
    conn: Connection,
    current: Option<i64>,
    current_last: i64,
    pending: Vec<Sample>,
    gap_secs: i64,
    config_snapshot: Option<String>,
}

#[derive(Clone)]
pub struct Storage {
    inner: Arc<Mutex<Inner>>,
}

pub type DbResult<T> = Result<T, rusqlite::Error>;

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS sessions(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL DEFAULT '',
  started INTEGER NOT NULL,
  last INTEGER NOT NULL,
  config TEXT
);
CREATE TABLE IF NOT EXISTS samples(
  session INTEGER NOT NULL,
  t INTEGER NOT NULL,
  sp REAL, t0 REAL, t1 REAL, t2 REAL, t3 REAL, out REAL, fan REAL, servo REAL,
  PRIMARY KEY(session, t)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS auth_sessions(
  token TEXT PRIMARY KEY,
  expires INTEGER NOT NULL
);
";

impl Storage {
    pub fn open(path: &Path, gap_hours: u32) -> DbResult<Storage> {
        let conn = Connection::open(path)?;
        Self::init(conn, gap_hours)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> DbResult<Storage> {
        Self::init(Connection::open_in_memory()?, 6)
    }

    fn init(conn: Connection, gap_hours: u32) -> DbResult<Storage> {
        conn.execute_batch(SCHEMA)?;
        let latest: Option<(i64, i64)> = conn
            .query_row("SELECT id, last FROM sessions ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(Storage {
            inner: Arc::new(Mutex::new(Inner {
                conn,
                current: latest.map(|l| l.0),
                current_last: latest.map(|l| l.1).unwrap_or(0),
                pending: Vec::new(),
                gap_secs: i64::from(gap_hours.max(1)) * 3600,
                config_snapshot: None,
            })),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Latest HeaterMeter configuration JSON, stored with the session.
    pub fn set_config_snapshot(&self, json: String) {
        self.lock().config_snapshot = Some(json);
    }

    /// Queue a sample. Starts a new session if there was a long gap or the
    /// clock jumped backwards.
    pub fn add(&self, s: Sample) {
        let mut g = self.lock();
        let gap = g.gap_secs;
        let new_session = match g.current {
            None => true,
            Some(_) => s.time - g.current_last > gap || s.time < g.current_last - 60,
        };
        if new_session {
            if let Err(e) = flush_locked(&mut g) {
                tracing::error!("flush failed: {e}");
            }
            let snapshot = g.config_snapshot.clone();
            match g.conn.execute(
                "INSERT INTO sessions(name, started, last, config) VALUES ('', ?1, ?1, ?2)",
                params![s.time, snapshot],
            ) {
                Ok(_) => {
                    let id = g.conn.last_insert_rowid();
                    tracing::info!("started history session {id}");
                    g.current = Some(id);
                }
                Err(e) => tracing::error!("cannot create session: {e}"),
            }
        }
        g.current_last = s.time;
        g.pending.push(s);
    }

    /// Write buffered samples.
    pub fn flush(&self) -> DbResult<()> {
        flush_locked(&mut self.lock())
    }

    /// Start a new session at the next sample.
    pub fn new_session(&self) -> DbResult<()> {
        let mut g = self.lock();
        flush_locked(&mut g)?;
        g.current = None;
        Ok(())
    }

    /// Continue appending to an existing session.
    pub fn continue_session(&self, id: i64) -> DbResult<bool> {
        let mut g = self.lock();
        flush_locked(&mut g)?;
        let last: Option<i64> =
            g.conn.query_row("SELECT last FROM sessions WHERE id=?1", [id], |r| r.get(0)).optional()?;
        match last {
            Some(last) => {
                g.current = Some(id);
                g.current_last = last;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn current_session(&self) -> Option<i64> {
        self.lock().current
    }

    pub fn sessions(&self) -> DbResult<Vec<SessionInfo>> {
        let mut g = self.lock();
        flush_locked(&mut g)?;
        let cur = g.current;
        let mut st = g.conn.prepare(
            "SELECT s.id, s.name, s.started, s.last, (SELECT count(*) FROM samples WHERE session=s.id)
             FROM sessions s ORDER BY s.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(SessionInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                started: r.get(2)?,
                last: r.get(3)?,
                samples: r.get(4)?,
                current: Some(r.get::<_, i64>(0)?) == cur,
            })
        })?;
        rows.collect()
    }

    pub fn rename(&self, id: i64, name: &str) -> DbResult<bool> {
        let g = self.lock();
        Ok(g.conn.execute("UPDATE sessions SET name=?2 WHERE id=?1", params![id, name])? > 0)
    }

    pub fn delete(&self, id: i64) -> DbResult<bool> {
        let mut g = self.lock();
        flush_locked(&mut g)?;
        let tx = g.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM samples WHERE session=?1", [id])?;
        let n = tx.execute("DELETE FROM sessions WHERE id=?1", [id])?;
        tx.commit()?;
        if g.current == Some(id) {
            g.current = None;
        }
        Ok(n > 0)
    }

    /// Time range `(first, last)` of a session.
    pub fn range(&self, id: i64) -> DbResult<Option<(i64, i64)>> {
        let mut g = self.lock();
        flush_locked(&mut g)?;
        g.conn.query_row("SELECT min(t), max(t) FROM samples WHERE session=?1", [id], |r| {
            Ok(match (r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => None,
            })
        })
    }

    /// Averaged history of a session between `from` and `to` (inclusive)
    /// in `step` second buckets. With `fill`, missing buckets are emitted as
    /// all-`None` rows (like rrdtool did).
    pub fn history(&self, id: i64, from: i64, to: i64, step: i64, fill: bool) -> DbResult<History> {
        let step = step.max(1);
        let mut g = self.lock();
        flush_locked(&mut g)?;
        let mut st = g.conn.prepare_cached(
            "SELECT (t/?2)*?2 AS b, avg(sp), avg(t0), avg(t1), avg(t2), avg(t3), avg(out), avg(fan), avg(servo)
             FROM samples WHERE session=?1 AND t>=?3 AND t<=?4 GROUP BY b ORDER BY b",
        )?;
        let mut h = History { step, ..Default::default() };
        let mut next: Option<i64> = None;
        let mut rows = st.query(params![id, step, from, to])?;
        while let Some(r) = rows.next()? {
            let b: i64 = r.get(0)?;
            if fill && let Some(mut n) = next {
                while n < b {
                    h.push_row(n, [None; 8]);
                    n += step;
                }
            }
            let mut v = [None; 8];
            for (i, slot) in v.iter_mut().enumerate() {
                *slot = r.get::<_, Option<f64>>(i + 1)?;
            }
            h.push_row(b, v);
            next = Some(b + step);
        }
        Ok(h)
    }

    /// Legacy `/luci/lm/hist` behaviour: pick the smallest of the four old
    /// RRA resolutions (1h@10s, 6h@60s, 12h@120s, 24h@180s) that shows the
    /// whole current session, or honour the `nancnt` override.
    pub fn legacy_history(&self, session: Option<i64>, nancnt: Option<i64>) -> DbResult<Option<History>> {
        let Some(id) = session.or_else(|| self.current_session()) else { return Ok(None) };
        let Some((first, last)) = self.range(id)? else { return Ok(None) };
        let span = last - first;
        let (step, soff) = match nancnt {
            Some(n) if n >= 460 => (10, 3600),
            Some(n) if n >= 360 => (60, 21600),
            Some(n) if n >= 240 => (120, 43200),
            Some(_) => (180, 86400),
            None if span <= 3600 => (10, 3600),
            None if span <= 21600 => (60, 21600),
            None if span <= 43200 => (120, 43200),
            None => (180, 86400),
        };
        // Buckets are aligned by the query itself; include the newest sample
        self.history(id, last - soff, last, step, true).map(Some)
    }

    // ---------------------------------------------------------- auth sessions

    pub fn auth_create(&self, token: &str, expires: i64) -> DbResult<()> {
        let g = self.lock();
        g.conn.execute("DELETE FROM auth_sessions WHERE expires < ?1", [expires - 86400 * 30])?;
        g.conn
            .execute("INSERT OR REPLACE INTO auth_sessions(token, expires) VALUES (?1, ?2)", params![token, expires])?;
        Ok(())
    }

    pub fn auth_check(&self, token: &str, now: i64) -> DbResult<bool> {
        let g = self.lock();
        let exp: Option<i64> =
            g.conn.query_row("SELECT expires FROM auth_sessions WHERE token=?1", [token], |r| r.get(0)).optional()?;
        Ok(exp.is_some_and(|e| e > now))
    }

    pub fn auth_delete(&self, token: &str) -> DbResult<()> {
        self.lock().conn.execute("DELETE FROM auth_sessions WHERE token=?1", [token])?;
        Ok(())
    }

    pub fn auth_clear(&self) -> DbResult<()> {
        self.lock().conn.execute("DELETE FROM auth_sessions", [])?;
        Ok(())
    }
}

fn flush_locked(g: &mut Inner) -> DbResult<()> {
    if g.pending.is_empty() {
        return Ok(());
    }
    let Some(id) = g.current else {
        g.pending.clear();
        return Ok(());
    };
    let pending = std::mem::take(&mut g.pending);
    let snapshot = g.config_snapshot.clone();
    let tx = g.conn.unchecked_transaction()?;
    {
        let mut st = tx.prepare_cached(
            "INSERT OR REPLACE INTO samples(session,t,sp,t0,t1,t2,t3,out,fan,servo) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        )?;
        for s in &pending {
            st.execute(params![
                id, s.time, s.sp, s.temps[0], s.temps[1], s.temps[2], s.temps[3], s.out, s.fan, s.servo
            ])?;
        }
    }
    let last = pending.last().map(|s| s.time).unwrap_or(g.current_last);
    tx.execute(
        "UPDATE sessions SET last=max(last, ?2), config=coalesce(?3, config) WHERE id=?1",
        params![id, last, snapshot],
    )?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: i64, pit: f64) -> Sample {
        Sample {
            time: t,
            sp: Some(225.0),
            temps: [Some(pit), None, None, None],
            out: Some(50.0),
            fan: None,
            servo: None,
        }
    }

    #[test]
    fn sessions_and_history() {
        let s = Storage::open_in_memory().unwrap();
        for i in 0..100 {
            s.add(sample(1000 + i, 200.0 + i as f64));
        }
        let list = s.sessions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].samples, 100);
        assert!(list[0].current);
        let h = s.history(list[0].id, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(h.len(), 10);
        assert_eq!(h.time[0], 1000);
        assert_eq!(h.t0[0], Some(204.5));
        assert_eq!(h.t1[0], None);

        // long gap -> new session
        s.add(sample(1000 + 7 * 3600, 100.0));
        let list = s.sessions().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].current && !list[1].current);
        assert!(s.rename(list[1].id, "Brisket").unwrap());
        assert!(s.delete(list[0].id).unwrap());
        assert_eq!(s.sessions().unwrap()[0].name, "Brisket");
    }

    #[test]
    fn legacy_csv() {
        let s = Storage::open_in_memory().unwrap();
        for i in 0..30 {
            s.add(sample(1_000_000 + i, 225.0));
        }
        s.add(sample(1_000_000 + 60, 225.0)); // gap -> nan rows filled
        let h = s.legacy_history(None, None).unwrap().unwrap();
        assert_eq!(h.step, 10);
        let csv = h.to_csv(false);
        let first_data = csv.lines().find(|l| l.contains("225")).unwrap();
        assert_eq!(first_data, "1000000,225,225,nan,nan,nan,50,nan,nan");
        assert!(csv.lines().any(|l| l.starts_with("1000030,nan")));
    }

    #[test]
    fn auth_tokens() {
        let s = Storage::open_in_memory().unwrap();
        s.auth_create("abc", 2000).unwrap();
        assert!(s.auth_check("abc", 1000).unwrap());
        assert!(!s.auth_check("abc", 3000).unwrap());
        s.auth_delete("abc").unwrap();
        assert!(!s.auth_check("abc", 1000).unwrap());
    }
}
