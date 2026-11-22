//! `hm-sim`: run the HeaterMeter simulator on a pseudo terminal.
//!
//! Prints the pty path; point linkmeterd at it with `serial_device = "<path>"`.

#[cfg(unix)]
fn main() {
    use std::ffi::CStr;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let speed: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);

    // SAFETY: plain libc calls creating a pty master and looking up its peer name
    let (master, name) = unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt failed");
        assert_eq!(libc::grantpt(fd), 0);
        assert_eq!(libc::unlockpt(fd), 0);
        let mut t: libc::termios = std::mem::zeroed();
        libc::tcgetattr(fd, &mut t);
        libc::cfmakeraw(&mut t);
        libc::tcsetattr(fd, libc::TCSANOW, &t);
        let name = CStr::from_ptr(libc::ptsname(fd)).to_string_lossy().into_owned();
        (File::from_raw_fd(fd), name)
    };
    println!("{name}");
    std::io::stdout().flush().ok();

    let mut sim = hm_sim::Simulator::new();
    sim.time_scale = speed;
    let sim = Arc::new(Mutex::new(sim));
    let mut writer = master.try_clone().expect("clone pty");
    let mut reader = master;

    {
        let sim = sim.clone();
        let mut w = writer.try_clone().expect("clone pty");
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => std::thread::sleep(Duration::from_millis(200)),
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let out = sim.lock().unwrap().receive(&s);
                        let _ = w.write_all(out.as_bytes());
                    }
                }
            }
        });
    }

    let boot = sim.lock().unwrap().boot();
    let _ = writer.write_all(boot.as_bytes());
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let out = sim.lock().unwrap().tick();
        let _ = writer.write_all(out.as_bytes());
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("hm-sim pty mode is only supported on unix");
}
