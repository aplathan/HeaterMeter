//! Network discovery: SSDP (UPnP) announcements compatible with the old
//! `ssdp-notify` package, plus the IP watcher that shows the address on the
//! HeaterMeter LCD. mDNS is provided by Avahi (see packaging/).

use crate::app::{AppRef, hostname, primary_ip};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;

pub fn device_xml(app: &AppRef, base: &str) -> String {
    let host = xml_escape(&hostname());
    format!(
        r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<URLBase>{base}</URLBase>
<device>
<deviceType>urn:schemas-upnp-org:device:Basic:1</deviceType>
<friendlyName>{host} (HeaterMeter)</friendlyName>
<manufacturer>HeaterMeter</manufacturer>
<manufacturerURL>https://github.com/CapnBry/HeaterMeter</manufacturerURL>
<modelDescription>HeaterMeter BBQ Controller</modelDescription>
<modelName>HeaterMeter</modelName>
<modelNumber>{ver}</modelNumber>
<UDN>uuid:{uuid}</UDN>
<presentationURL>/</presentationURL>
</device>
</root>
"#,
        ver = env!("CARGO_PKG_VERSION"),
        uuid = app.uuid
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn notify_msg(location: &str, uuid: &str, nt: &str) -> String {
    let usn = if nt.starts_with("uuid:") { nt.to_string() } else { format!("uuid:{uuid}::{nt}") };
    format!(
        "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nCACHE-CONTROL: max-age=120\r\nLOCATION: {location}\r\nNT: {nt}\r\nNTS: ssdp:alive\r\nSERVER: Linux UPnP/1.0 HeaterMeter/{}\r\nUSN: {usn}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )
}

fn search_reply(location: &str, uuid: &str, st: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nEXT:\r\nLOCATION: {location}\r\nSERVER: Linux UPnP/1.0 HeaterMeter/{}\r\nST: {st}\r\nUSN: uuid:{uuid}::{st}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )
}

fn http_port(listen: &str) -> u16 {
    listen.parse::<SocketAddr>().map(|a| a.port()).unwrap_or(80)
}

fn location(ip: &str, port: u16) -> String {
    if port == 80 { format!("http://{ip}/luci/ssdp") } else { format!("http://{ip}:{port}/luci/ssdp") }
}

pub async fn ssdp_task(app: AppRef) {
    let port = http_port(&app.config().daemon.listen);
    let listener = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, SSDP_PORT)).await {
        Ok(s) => {
            if let Err(e) = s.join_multicast_v4(SSDP_ADDR, Ipv4Addr::UNSPECIFIED) {
                tracing::debug!("ssdp multicast join failed: {e}");
            }
            Some(s)
        }
        Err(e) => {
            tracing::info!("SSDP search responder disabled: {e}");
            None
        }
    };
    let sender = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("SSDP disabled: {e}");
            return;
        }
    };
    let _ = sender.set_multicast_ttl_v4(2);
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut buf = vec![0u8; 2048];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(ip) = primary_ip() {
                    let loc = location(&ip, port);
                    let uuid_nt = format!("uuid:{}", app.uuid);
                    for nt in ["upnp:rootdevice", uuid_nt.as_str()] {
                        let _ = sender.send_to(notify_msg(&loc, &app.uuid, nt).as_bytes(), (SSDP_ADDR, SSDP_PORT)).await;
                    }
                }
            }
            r = async {
                match &listener {
                    Some(l) => l.recv_from(&mut buf).await,
                    None => std::future::pending().await,
                }
            } => {
                let Ok((n, from)) = r else { continue };
                let msg = String::from_utf8_lossy(&buf[..n]);
                if !msg.starts_with("M-SEARCH") { continue; }
                let st = msg.lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("ST")).map(|(_, v)| v.trim().to_string()))
                    .unwrap_or_default();
                if (st == "ssdp:all" || st == "upnp:rootdevice")
                    && let Some(ip) = primary_ip() {
                        let reply = search_reply(&location(&ip, port), &app.uuid, "upnp:rootdevice");
                        let _ = sender.send_to(reply.as_bytes(), from).await;
                    }
            }
        }
    }
}

/// Show the IP address on the LCD whenever it changes (ipwatch.lua).
pub async fn ipwatch_task(app: AppRef) {
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        tick.tick().await;
        let ip = primary_ip();
        let wifi = if app.config().wifi.enabled { crate::wifi::lcd_line(&app).await } else { None };
        let ap = app.wifi.ap_active();
        app.device.with_core(|c| {
            c.netinfo.wifi = wifi;
            c.netinfo.ap_active = ap;
            ((), c.set_ip(ip))
        });
    }
}

/// Stable device UUID persisted in the data directory.
pub fn load_uuid(dir: &std::path::Path) -> String {
    let path = dir.join("uuid");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() == 36 {
            return s;
        }
    }
    let h = crate::app::random_hex(16);
    let u = format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32]);
    let _ = std::fs::write(&path, &u);
    u
}
