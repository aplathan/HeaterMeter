use super::*;
use hm_protocol::encode_line;

pub(crate) const CONFIG_DUMP: &[&str] = &[
    "UCID,HeaterMeter,20210202B",
    "HMPD,0,4.00000000,0.02000000,5.00000000,F",
    "HMFN,0,100,100,200,0,100,0,100",
    "HMPN,Pit,Food Probe1,Food Probe2,Ambient",
    "HMPC,0,2.3067434e-4,2.3696596e-4,1.2636414e-7,1.0000000e+4,1",
    "HMPC,1,2.4723753e-4,2.3402251e-4,1.3879768e-7,1.0000000e+4,1",
    "HMPC,2,2.4723753e-4,2.3402251e-4,1.3879768e-7,1.0000000e+4,1",
    "HMPC,3,2.4723753e-4,2.3402251e-4,1.3879768e-7,1.0000000e+4,0",
    "HMPO,0,0,0,0",
    "HMLD,6,240",
    "HMLB,50,255,10,2,1,127",
    "HMAL,-40,-200,-40,-200,-40,-200,-40,-200",
    "HMRM,,,,",
];

fn feed(core: &mut Core, payload: &str, now: i64) -> Vec<Effect> {
    core.on_line(encode_line(payload).trim_end(), now)
}

pub(crate) fn configured_core() -> Core {
    let mut c = Core::new(Settings::default(), RampParams::default());
    for (i, l) in CONFIG_DUMP.iter().enumerate() {
        feed(&mut c, l, 1000 + i as i64 / 10);
    }
    c
}

fn serial(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Serial(c) => Some(c.text()),
            _ => None,
        })
        .collect()
}

fn broadcasts<'a>(fx: &'a [Effect], name: &str) -> Vec<&'a str> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Broadcast { name: n, data } if *n == name => Some(data.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn first_line_requests_config() {
    let mut c = Core::new(Settings::default(), RampParams::default());
    let fx = feed(&mut c, "HMSU,225,U,U,U,U,0,0,0,0,0", 1000);
    assert_eq!(serial(&fx), vec!["/config"]);
    assert!(!c.config_complete());
    // no further request within 2 seconds
    assert!(serial(&c.on_tick(1002)).is_empty());
    // retransmit after > 2 seconds
    assert_eq!(serial(&c.on_tick(1003)), vec!["/config"]);
}

#[test]
fn config_complete() {
    let c = configured_core();
    assert!(c.config_complete(), "missing {:?}", c.missing_config());
    assert_eq!(c.ucid().as_deref(), Some("20210202B"));
    let cfg: serde_json::Value = serde_json::from_str(&c.config_json()).unwrap();
    assert_eq!(cfg["pidp"], 4);
    assert_eq!(cfg["pidi"], 0.02);
    assert_eq!(cfg["u"], "F");
    assert_eq!(cfg["pn0"], "Pit");
    assert_eq!(cfg["pcr0"], 10000);
    assert_eq!(cfg["pca0"], "2.3067434e-4");
    assert_eq!(cfg["lb"], "50");
    assert_eq!(cfg["pall0"], -40);
    assert_eq!(cfg["palh3"], -200);
    assert_eq!(cfg["pcurr0"], serde_json::Value::Null);
    assert_eq!(cfg["prfn0"], "");
}

#[test]
fn bad_checksum_counted() {
    let mut c = configured_core();
    c.on_line("$HMSU,225,U,U,U,U,0,0,0,0,0*00", 2000);
    assert_eq!(c.checksum_errors, 1);
    let cfg: serde_json::Value = serde_json::from_str(&c.config_json()).unwrap();
    assert_eq!(cfg["cerr"], 1);
}

#[test]
fn status_update_flow() {
    let mut c = configured_core();
    let fx = feed(&mut c, "HMSU,225,200.5,150.0,U,U,40,35,0,40,0", 2000);
    let st = broadcasts(&fx, "hmstatus");
    assert_eq!(st.len(), 1);
    let v: serde_json::Value = serde_json::from_str(st[0]).unwrap();
    assert_eq!(v["time"], 2000);
    assert_eq!(v["set"], 225);
    assert_eq!(v["temps"][0]["c"], 200.5);
    assert_eq!(v["temps"][1]["c"], 150);
    assert_eq!(v["temps"][2]["c"], serde_json::Value::Null);
    assert_eq!(v["temps"][0]["a"]["l"], -40);
    assert_eq!(v["fan"]["f"], 40);
    let sample = fx.iter().find_map(|e| if let Effect::Sample(s) = e { Some(s.clone()) } else { None }).unwrap();
    assert_eq!(sample.sp, Some(225.0));
    assert_eq!(sample.out, Some(40.0));
    assert_eq!(sample.temps[2], None);
}

#[test]
fn manual_mode_and_lid() {
    let mut c = configured_core();
    let fx = feed(&mut c, "HMSU,-,200,U,U,U,60,50,15,60,0", 2000);
    let v: serde_json::Value = serde_json::from_str(broadcasts(&fx, "hmstatus")[0]).unwrap();
    assert_eq!(v["set"], -60);
    assert_eq!(v["lid"], 15);
    let sample = fx.iter().find_map(|e| if let Effect::Sample(s) = e { Some(s.clone()) } else { None }).unwrap();
    assert_eq!(sample.sp, None);
    assert_eq!(sample.out, Some(-15.0));
}

#[test]
fn old_firmware_short_status() {
    let mut c = configured_core();
    let fx = feed(&mut c, "HMSU,225,200,U,U,U,60,50,0", 2000);
    let v: serde_json::Value = serde_json::from_str(broadcasts(&fx, "hmstatus")[0]).unwrap();
    assert_eq!(v["fan"]["f"], serde_json::Value::Null);
}

#[test]
fn duplicate_timestamp_discarded() {
    let mut c = configured_core();
    let fx1 = feed(&mut c, "HMSU,225,200,U,U,U,60,50,0,60,0", 2000);
    let fx2 = feed(&mut c, "HMSU,225,201,U,U,U,60,50,0,60,0", 2000);
    assert_eq!(broadcasts(&fx1, "hmstatus").len(), 1);
    assert_eq!(broadcasts(&fx2, "hmstatus").len(), 0);
}

#[test]
fn alarm_ringing_edge() {
    let mut c = configured_core();
    feed(&mut c, "HMSU,225,200,180,U,U,40,35,0,40,0", 2000);
    let fx = feed(&mut c, "HMAL,-40,-200,-40,170H,-40,-200,-40,-200", 2001);
    let starts: Vec<&AlarmEvent> =
        fx.iter().filter_map(|e| if let Effect::AlarmStart(a) = e { Some(a) } else { None }).collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].probe, 1);
    assert_eq!(starts[0].kind, 'H');
    assert_eq!(starts[0].env["al_prep"], "above");
    assert_eq!(starts[0].env["pn"], "Food Probe1");
    assert_eq!(starts[0].env["pcurr"], "180");
    assert_eq!(broadcasts(&fx, "alarm")[0], "{\"atype\":\"H\",\"p\":1,\"pn\":\"Food Probe1\",\"c\":180,\"t\":170}");
    assert_eq!(c.status.probes[1].ringing, Some('H'));
    // still ringing: no new event
    let fx = feed(&mut c, "HMAL,-40,-200,-40,170H,-40,-200,-40,-200", 2002);
    assert!(!fx.iter().any(|e| matches!(e, Effect::AlarmStart(_))));
    // silenced
    let fx = feed(&mut c, "HMAL,-40,-200,-40,170,-40,-200,-40,-200", 2003);
    assert_eq!(broadcasts(&fx, "alarm")[0], "{\"atype\":null,\"p\":1,\"pn\":\"Food Probe1\",\"c\":180,\"t\":170}");
    assert_eq!(c.status.probes[1].ringing, None);
}

#[test]
fn alarm_needs_ucid() {
    let mut c = Core::new(Settings::default(), RampParams::default());
    let fx = feed(&mut c, "HMAL,-40,200H,-40,-200,-40,-200,-40,-200", 1000);
    assert!(!fx.iter().any(|e| matches!(e, Effect::AlarmStart(_))));
}

#[test]
fn set_validation() {
    let mut c = configured_core();
    assert_eq!(serial(&c.set("sp", "250").unwrap()), vec!["/set?sp=250"]);
    assert!(c.set("s p", "1").is_err());
    assert!(c.set("sp", "1\n/reboot").is_err());
    assert!(c.set("tt", &"x".repeat(70)).is_err());
}

#[test]
fn alarm_set_positional() {
    assert_eq!(alarm_set_value(0, 'L', "0"), "0");
    assert_eq!(alarm_set_value(1, 'H', "-170"), ",,,-170");
    assert_eq!(alarm_set_value(3, 'H', "0"), ",,,,,,,0");
}

#[test]
fn rf_status() {
    let mut c = configured_core();
    feed(&mut c, "HMRM,,5,,", 2000);
    feed(&mut c, "HMRF,255,0,12,5,1,-70", 2001);
    let rf: serde_json::Value = serde_json::from_str(&c.rf_json()).unwrap();
    assert_eq!(rf.as_array().unwrap().len(), 3); // 127 alias, 255, 5
    assert_eq!(c.status.probes[1].ext.get("rf"), Some("{\"s\":-70,\"b\":1}"));
    feed(&mut c, "HMRF,255,0,12", 2002);
    assert_eq!(c.status.probes[1].ext.get("rf"), Some("0"));
}

#[test]
fn reboot_resets() {
    let mut c = configured_core();
    let fx = c.reboot();
    assert_eq!(serial(&fx), vec!["/reboot"]);
    assert!(!c.has_config());
    assert_eq!(c.status_json(), "{}");
}

#[test]
fn host_interactive_netinfo() {
    let mut c = configured_core();
    c.set_ip(Some("192.168.1.50".into()));
    let fx = feed(&mut c, "HMHI,0,0,8", 2000); // DOWN from title
    assert_eq!(serial(&fx), vec!["/set?hi=1,Network Address,192.168.1.50"]);
}

#[test]
fn ramp_runs() {
    let mut c = Core::new(
        Settings::default(),
        RampParams { enabled: true, watch: vec![1], trigger: 150.0, target: 200.0, start_setpoint: None },
    );
    for (i, l) in CONFIG_DUMP.iter().enumerate() {
        feed(&mut c, l, 1000 + i as i64 / 10);
    }
    // below trigger: nothing
    let fx = feed(&mut c, "HMSU,250,250,140,U,U,40,35,0,40,0", 2000);
    assert!(serial(&fx).is_empty());
    // above trigger: start, setpoint moves toward target
    let fx = feed(&mut c, "HMSU,250,250,175,U,U,40,35,0,40,0", 2001);
    assert!(fx.iter().any(|e| matches!(e, Effect::RampStarted { start_setpoint } if *start_setpoint == 250.0)));
    // progress = (200-175)/(200-150) = 0.5 -> sp = ceil(50*0.5)+200 = 225
    assert_eq!(serial(&fx), vec!["/set?sp=225"]);
    // the requested setpoint arriving back must not cancel
    let fx = feed(&mut c, "HMSU,225,250,176,U,U,40,35,0,40,0", 2002);
    assert!(!fx.iter().any(|e| matches!(e, Effect::RampCancelled)));
    // reaching target completes the ramp
    let fx = feed(&mut c, "HMSU,225,250,199.5,U,U,40,35,0,40,0", 2003);
    assert!(fx.iter().any(|e| matches!(e, Effect::RampCancelled)));
    assert_eq!(serial(&fx), vec!["/set?sp=200"]);
}

#[test]
fn ramp_cancel_on_user_change() {
    let mut c = Core::new(
        Settings::default(),
        RampParams { enabled: true, watch: vec![1], trigger: 150.0, target: 200.0, start_setpoint: None },
    );
    for (i, l) in CONFIG_DUMP.iter().enumerate() {
        feed(&mut c, l, 1000 + i as i64 / 10);
    }
    feed(&mut c, "HMSU,250,250,175,U,U,40,35,0,40,0", 2001);
    let fx = feed(&mut c, "HMSU,300,250,175,U,U,40,35,0,40,0", 2002);
    assert!(fx.iter().any(|e| matches!(e, Effect::RampCancelled)));
}

#[test]
fn dph_estimate() {
    let mut c = configured_core();
    let mut last = Vec::new();
    // probe 1 rises 30 degrees per hour, sampled each second for an hour
    for s in 0..3700 {
        let t1 = 100.0 + 30.0 * s as f64 / 3600.0;
        last = feed(&mut c, &format!("HMSU,225,225,{t1:.1},U,U,40,35,0,40,0"), 10_000 + s);
    }
    let _ = last;
    let dph = c.status.probes[1].ext.get("dph").unwrap();
    let dph: f64 = dph.parse().unwrap();
    assert!((dph - 30.0).abs() < 0.5, "dph {dph}");
    assert_eq!(c.status.probes[2].ext.get("dph"), None);
}

#[test]
fn lidtrack_triggers() {
    let mut c = Core::new(Settings { lidtrack_enabled: true }, RampParams::default());
    for (i, l) in CONFIG_DUMP.iter().enumerate() {
        feed(&mut c, l, 1000 + i as i64 / 10);
    }
    let mut t = 2000;
    // steady at setpoint
    for _ in 0..60 {
        feed(&mut c, "HMSU,225,226,U,U,U,40,35,0,40,0", t);
        t += 1;
    }
    // lid opened: fast drop
    let mut triggered = false;
    for k in 0..20 {
        let temp = 226.0 - 3.0 * k as f64;
        let fx = feed(&mut c, &format!("HMSU,225,{temp:.1},U,U,U,40,35,0,40,0"), t);
        t += 1;
        if serial(&fx).iter().any(|s| s == "/set?ld=,,1") {
            triggered = true;
            break;
        }
    }
    assert!(triggered);
}

#[test]
fn unkprobe_flow() {
    let mut c = configured_core();
    let (_, fx) = c.with_ctx(|p, ctx| p.unkprobe.start(ctx));
    assert_eq!(serial(&fx), vec!["/set?sp=0R"]);
    let truth = [7.3431401e-4f64, 2.1574370e-4, 9.5156860e-8];
    for (now, ohms) in (3000..).zip([5_000.0f64, 10_000.0, 20_000.0, 40_000.0, 80_000.0, 160_000.0]) {
        let l = ohms.ln();
        let t = 1.0 / ((truth[2] * l * l + truth[1]) * l + truth[0]) - 273.15;
        feed(&mut c, &format!("HMSU,U,{t:.1},{ohms},U,U,0,0,0,0,0"), now);
    }
    let fit: serde_json::Value = serde_json::from_str(&c.unkprobe.fit_json().unwrap()).unwrap();
    assert_eq!(fit["n"], 5); // first point discarded
    let a = fit["a"].as_f64().unwrap();
    assert!((a - truth[0]).abs() / truth[0] < 0.05, "a={a}");
}

#[test]
fn toast_sanitized() {
    let c = configured_core();
    assert_eq!(serial(&c.toast("Hello, world", "x")), vec!["/set?tt=Hello  world,x"]);
}
