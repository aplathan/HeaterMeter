# HeaterMeter Neo

A modern replacement for **LinkMeter**, the Raspberry Pi side of
[HeaterMeter](https://github.com/CapnBry/HeaterMeter). The OpenWrt/LEDE image
(2017–18 kernel, Lua 5.1, LuCI, rrdtool 1.4, jQuery 1.9) is replaced by a
single Rust daemon with an embedded web UI. It runs on stock **Raspberry Pi OS
Lite** as a systemd service.

The HeaterMeter (AVR) firmware is **not changed**. The serial protocol and the
LinkMeter HTTP API are kept compatible, so existing HeaterMeter boards,
MeterMonitor, SMUSHI, the `HeaterMeterClient` library and third party apps
keep working.

```
HeaterMeter AVR ──UART 38400──▶ linkmeterd ──▶ web UI (Svelte + uPlot), /api/v2, legacy /luci/lm/*, SSE
                ◀──/set?...───      │      ──▶ SQLite history (1 Hz, per cook)
   SPI + GPIO25 ◀── avrdude ────────┘      ──▶ alarms: email, SMS, ntfy, Pushover, Pushbullet, webhook, scripts
```

## Layout

| Path | What |
|---|---|
| `crates/hm-protocol` | Serial line parsing, XOR checksum, command encoding |
| `crates/lm-core` | Device state machine, a port of `linkmeterd.lua`: config handshake, status JSON, throttling, alarms, and the peaks/LidTrack, ramp, degrees-per-hour, unknown-probe and LCD network-menu plugins |
| `crates/hm-sim` | HeaterMeter firmware simulator (thermal model). Used in-process (`--simulate`) or on a pty (`hm-sim` binary) |
| `crates/linkmeterd` | The daemon: serial I/O, HTTP, storage, notifications, AVR flashing, wifi, discovery |
| `web/` | Web UI (Svelte 5, TypeScript, uPlot), embedded into the binary at build time |
| `packaging/` | systemd unit, `.deb` metadata, Avahi service, pi-gen stage, cross build script |

## Quick start (no hardware needed)

```sh
cd web && npm install && npm run build && cd ..
cargo run -p linkmeterd -- --simulate --sim-speed 10 -l 127.0.0.1:8080 -d /tmp/lm -c /tmp/lm/config.toml
# open http://127.0.0.1:8080
```

For UI development, run the daemon as above on port 8099 and `npm run dev` in
`web/`. Vite proxies `/api` and `/luci` to it.

## Installing on a Raspberry Pi

1. Flash Raspberry Pi OS Lite (Bookworm or later). Set up wifi/SSH in Raspberry Pi Imager.
2. Install the package (from CI artifacts, or build it with `packaging/build.sh`):
   ```sh
   sudo apt install ./linkmeterd_*_arm64.deb avrdude gpiod
   sudo reboot   # the postinst enables the UART and SPI
   ```
3. Open `http://heatermeter.local/` (or the Pi's IP) and set an admin password
   under **Settings → System**.

A ready-to-flash image can be built with [pi-gen](https://github.com/RPi-Distro/pi-gen)
using `packaging/pi-gen/` (see the comments in `packaging/pi-gen/config`).

Configuration lives in `/etc/linkmeter/config.toml`. Most of it is edited
from the web UI. History is stored in `/var/lib/linkmeter/history.db`.

### Migrating from the OpenWrt LinkMeter

* HeaterMeter's own settings (probes, PID, alarms thresholds…) live in the AVR
  EEPROM and carry over automatically.
* LinkMeter settings (alarm actions, email/push, ramp, API key) can be
  imported from an OpenWrt backup:
  `sudo linkmeterd import-uci etc/config/linkmeter`.
* Old `.rrd` archives are not imported; download them as CSV from the old
  device first if you want to keep them.
* Alarm scripts (`alarm-all`, `alarm-1H`, …) go to `/etc/linkmeter/scripts/`
  and receive the same environment variables as before.

## Compatibility

These endpoints behave like the LuCI originals:

| Endpoint | Notes |
|---|---|
| `GET /luci/lm/stream` | SSE `hmstatus`, `log`, `noisedump`, `pidint`, `alarm`, `peaks`. Accepts requests without a `Host` header. Each event is sent as its own chunk, and status JSON stays under 1 KiB (HeaterMeterClient requirements). |
| `GET /luci/lm/hmstatus`, `/luci/lm/conf`, `/luci/lm/rfstatus` | Same JSON layout and number formatting |
| `GET /luci/lm/hist?nancnt=&dl=&hdr=` | Same CSV and resolution rules. `rrd=` now takes a session id instead of a file path. |
| `/luci/lm/api/{version,status/…,config/…,fw}` | API key handling, `api.disabled`, `api.allowcors` |
| `/luci/admin/lm/set` | `rawset`, `sp` `%`/`OFF` conversion, `lm_apikey`, `lm_lidtrack_enabled`. Needs a login or the API key. |
| `/luci/ssdp` + SSDP NOTIFY, mDNS `_http._tcp path=/luci/lm/` | Discovery |

The new UI uses `/api/v2/*` (JSON). It is documented by
`crates/linkmeterd/src/http/api.rs`.

## Changes compared to LinkMeter

* **History:** 1 Hz samples per cook in SQLite, with no 24 h limit.
  Sessions can be named, continued, deleted and exported to CSV.
* **Notifications:** ntfy and generic webhooks were added. Templates use
  `$var` substitution without shell `eval`. Notifications can be tested
  directly from the UI.
* **Login:** a dedicated admin password (argon2) with session cookies.
  Read-only guest access can be switched off.
* **AVR flashing:** stock `avrdude -c linuxspi` (GPIO character device)
  replaces `hmdude`/`avrupdate`. The fuse handling, blank-chip programming
  and auto-recovery are ported.
* **Wifi:** managed through NetworkManager. A fallback access point
  (`heatermeter`) starts if no network is available after boot.
* **Factory reset:** a reset from the HeaterMeter LCD (`$HMRC`) resets
  LinkMeter settings only, not the whole OS.
* **Not implemented:** heatermeter.com device registration and the
  heatermeter.com SMTP relay (configure your own SMTP server), and import
  of old `.rrd` files.

## Status

The protocol, core logic, storage, HTTP API (including a raw-socket
HeaterMeterClient compatibility test) and the UI are tested against the
simulator (`cargo test --workspace`). The hardware paths have **not yet
been verified on a real Raspberry Pi with a HeaterMeter board**:

* the serial port on `/dev/serial0`
* avrdude `linuxspi` flashing
* NetworkManager wifi control
