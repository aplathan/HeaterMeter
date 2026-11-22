//! AVR (ATmega328P) flashing over SPI with stock avrdude, replacing the
//! OpenWrt `avrupdate` script and the custom `hmdude` tool.
//!
//! avrdude's `linuxspi` programmer drives `/dev/spidev0.0` and toggles the
//! AVR reset line (BCM GPIO 25) through the GPIO character device.

use crate::app::AppRef;
use crate::config::FirmwareConfig;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// Where the hex to flash comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Already on disk (uploaded or bundled)
    File(PathBuf),
    /// Download first
    Url(String),
}

pub type Out = mpsc::UnboundedSender<String>;

fn say(out: &Out, s: impl Into<String>) {
    let _ = out.send(s.into());
}

/// Find the GPIO chip that carries the 40-pin header lines.
fn gpio_chip(cfg: &FirmwareConfig) -> String {
    if cfg.gpio_chip != "auto" {
        return cfg.gpio_chip.clone();
    }
    // Pi 5 (RP1) and Pi 0-4 (bcm2835/2711) pinctrl labels
    if let Ok(o) = std::process::Command::new("gpiodetect").output() {
        for l in String::from_utf8_lossy(&o.stdout).lines() {
            if (l.contains("pinctrl-rp1") || l.contains("pinctrl-bcm"))
                && let Some(name) = l.split_whitespace().next()
            {
                return format!("/dev/{name}");
            }
        }
    }
    "/dev/gpiochip0".into()
}

fn port(cfg: &FirmwareConfig) -> String {
    format!("{}:{}:{}", cfg.spi_device, gpio_chip(cfg), cfg.reset_line)
}

/// Basic Intel HEX sanity check.
pub fn looks_like_hex(data: &[u8]) -> bool {
    let s = String::from_utf8_lossy(data);
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let all_ok = lines.clone().all(|l| {
        let l = l.trim();
        l.starts_with(':') && l.len() >= 11 && l[1..].bytes().all(|b| b.is_ascii_hexdigit())
    });
    all_ok && lines.next().is_some() && s.contains(":00000001FF")
}

async fn avrdude(cfg: &FirmwareConfig, speed_hz: u32, args: &[String], out: &Out) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(&cfg.avrdude);
    cmd.arg("-c").arg("linuxspi").arg("-P").arg(port(cfg)).arg("-p").arg("m328p").arg("-b").arg(speed_hz.to_string());
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("cannot run {}: {e}", cfg.avrdude))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let collected = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut handles = Vec::new();
    for pipe in [
        stdout.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        stderr.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let out = out.clone();
        let collected = collected.clone();
        handles.push(tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if let Ok(mut c) = collected.lock() {
                    c.push_str(&l);
                    c.push('\n');
                }
                // avrdude progress bars are noisy; keep meaningful lines
                if !l.trim().is_empty() {
                    let _ = out.send(l);
                }
            }
        }));
    }
    let status = tokio::time::timeout(std::time::Duration::from_secs(180), child.wait())
        .await
        .map_err(|_| "avrdude timed out".to_string())?
        .map_err(|e| e.to_string())?;
    for h in handles {
        let _ = h.await;
    }
    let text = collected.lock().map(|c| c.clone()).unwrap_or_default();
    if status.success() { Ok(text) } else { Err(format!("avrdude failed ({status})")) }
}

/// Read fuses as a lowercase hex string `lfuse hfuse efuse`, e.g. `ffd705`.
async fn read_fuses(cfg: &FirmwareConfig, out: &Out) -> Result<String, String> {
    let tmp = std::env::temp_dir();
    let files: Vec<PathBuf> = ["lfuse", "hfuse", "efuse"].iter().map(|f| tmp.join(format!("lm-{f}.hex"))).collect();
    let mut args = vec!["-qq".to_string()];
    for (f, p) in ["lfuse", "hfuse", "efuse"].iter().zip(&files) {
        args.push("-U".into());
        args.push(format!("{f}:r:{}:h", p.display()));
    }
    avrdude(cfg, 125_000, &args, out).await?;
    let mut s = String::new();
    for p in &files {
        let v = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(p);
        let n = u8::from_str_radix(v.trim().trim_start_matches("0x"), 16).map_err(|e| format!("fuse parse: {e}"))?;
        s.push_str(&format!("{n:02x}"));
    }
    Ok(s)
}

const GOOD_FUSES: [&str; 6] = ["-U", "lfuse:w:0xff:m", "-U", "hfuse:w:0xd7:m", "-U", "efuse:w:0x05:m"];

async fn download(app: &AppRef, url: &str, out: &Out) -> Result<PathBuf, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs are supported".into());
    }
    say(out, format!("Downloading {url}"));
    let resp = app.http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download failed: {}", resp.status()));
    }
    let data = resp.bytes().await.map_err(|e| e.to_string())?;
    if data.len() > 512 * 1024 || !looks_like_hex(&data) {
        return Err("downloaded file is not an Intel HEX firmware image".into());
    }
    let path = std::env::temp_dir().join("lm-download.hex");
    std::fs::write(&path, &data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Full update procedure. Output lines are streamed to `out`.
pub async fn update(app: &AppRef, source: Source, out: Out) -> bool {
    let cfg = app.config().firmware;
    if !cfg.enabled {
        say(&out, "AVR flashing is disabled ([firmware] enabled = false)");
        return false;
    }
    let Ok(_guard) = app.flash_lock.try_lock() else {
        say(&out, "Another firmware update is already running");
        return false;
    };
    let hex = match source {
        Source::Url(u) => match download(app, &u, &out).await {
            Ok(p) => Some(p),
            Err(e) => {
                say(&out, e);
                return false;
            }
        },
        Source::File(p) => Some(p),
    };

    say(&out, "Stopping LinkMeter serial connection");
    app.device.pause().await;
    let ok = flash_paused(&cfg, hex.as_deref(), &out).await;
    say(&out, "Starting LinkMeter serial connection");
    app.device.resume().await;
    say(&out, if ok { "Update successful" } else { "Update failed" });
    ok
}

async fn flash_paused(cfg: &FirmwareConfig, hex: Option<&Path>, out: &Out) -> bool {
    let fuses = match read_fuses(cfg, out).await {
        Ok(f) => f,
        Err(e) => {
            say(out, format!("AVR fuses ERROR: {e}"));
            return false;
        }
    };
    let mut hex = hex.map(Path::to_path_buf);
    match fuses.as_str() {
        "62d9ff" | "62d907" => {
            say(out, format!("AVR fuses {fuses}: blank ATmega chip detected, flashing default"));
            let mut args = vec!["-U".to_string(), format!("flash:w:{}:i", cfg.bundled_hex.display())];
            args.extend(GOOD_FUSES.iter().map(|s| s.to_string()));
            if let Err(e) = avrdude(cfg, 125_000, &args, out).await {
                say(out, e);
                return false;
            }
            if hex.as_deref() == Some(cfg.bundled_hex.as_path()) {
                hex = None;
            }
        }
        f if f.starts_with("ffdf") => {
            say(out, format!("AVR fuses {fuses}: Arduino fuses, switching to preserve EEPROM"));
            let args: Vec<String> = GOOD_FUSES.iter().map(|s| s.to_string()).collect();
            if let Err(e) = avrdude(cfg, 1_000_000, &args, out).await {
                say(out, e);
                return false;
            }
        }
        f => say(out, format!("AVR fuses {f} OK")),
    }
    let Some(hex) = hex else { return true };
    if !hex.exists() {
        say(out, format!("No input file {}", hex.display()));
        return false;
    }
    match std::fs::read(&hex) {
        Ok(d) if looks_like_hex(&d) => {}
        _ => {
            say(out, "Input is not an Intel HEX file");
            return false;
        }
    }
    say(out, format!("Flashing {}", hex.display()));
    match avrdude(cfg, 1_000_000, &["-U".to_string(), format!("flash:w:{}:i", hex.display())], out).await {
        Ok(_) => true,
        Err(e) => {
            say(out, e);
            false
        }
    }
}

/// Auto-recovery hook for the serial loop (port of linkmeterd `lmdTick`).
/// The serial connection is already closed when this runs.
pub async fn recover(app: AppRef) -> bool {
    let cfg = app.config().firmware;
    if !cfg.enabled || !cfg.auto_recover {
        return false;
    }
    let Ok(_guard) = app.flash_lock.try_lock() else { return false };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(l) = rx.recv().await {
            tracing::info!(target: "avrupdate", "{l}");
        }
    });
    let ok = flash_paused(&cfg, None, &tx).await;
    drop(tx);
    let _ = task.await;
    ok
}

/// Firmware files shipped with the package.
pub fn bundled(cfg: &FirmwareConfig) -> Vec<PathBuf> {
    let dir = cfg.bundled_hex.parent().unwrap_or(Path::new("/usr/share/linkmeter"));
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "hex")).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_check() {
        assert!(looks_like_hex(b":100000000C9435000C945D000C945D000C945D0024\n:00000001FF\n"));
        assert!(!looks_like_hex(b"<html>not found</html>"));
        assert!(!looks_like_hex(b""));
    }
}
