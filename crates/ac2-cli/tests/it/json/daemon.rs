//! Devices and daemon status.

use ac2_client::fake::{FakeDaemon, FakeOptions};
use serde_json::{Value, json};

use super::{R, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn devices_json() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["devices", "--json"]).await?;
    let dir = |ch, names: serde_json::Value| {
        json!({
            "max_channels": ch,
            "rates_hz": [{ "min": 48000, "max": 48000 }],
            "buffer_frames": { "min": 256, "max": 256 },
            "default_rate_hz": 48000,
            "default_buffer_frames": 256,
            "channel_names": names,
            "system_default": true
        })
    };
    assert_eq!(
        v,
        json!([
            {
                "kind": "fake",
                "description": "Simulated rig (no audio): out 1 returns on in 1 (loop) and in 2 (room)",
                "availability": { "type": "available" },
                "devices": [{
                    "backend": "fake",
                    "host": "fake",
                    "id": "fake:loop",
                    "name": "Fake loopback",
                    "input": dir(4, json!(["Loop return", "Room mic", "Line 3", "Line 4"])),
                    "output": dir(2, json!(["Out 1 (speaker + loop)", "Out 2"])),
                    "duplex_clock": "single_callback",
                    "index": "exact",
                    "notes": []
                }]
            },
            {
                "kind": "jack",
                "description": "JACK audio server",
                "availability": { "type": "unavailable", "reason": "JACK server not running" },
                "devices": []
            }
        ])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_build_ids() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["status", "--json"]).await?;
    assert_eq!(v["server"], "ac2d 0.0.0 (build fake)");
    assert_eq!(v["build_id"], "fake");
    assert_eq!(v["client_build_id"], ac2_cli::BUILD_ID);
    assert_eq!(v["build_matches"], false);
    // Not a local daemon: "stale" is not judged.
    assert_eq!(v["stale"], Value::Null);
    assert_eq!(v["daemon_incarnation"], "000000005eed0001");
    assert_eq!(
        v["session"],
        json!({ "epoch": 1, "open": null, "stopped": null })
    );
    assert_eq!(
        v["autosave"],
        json!({ "state": { "type": "off" }, "saved_at": null })
    );
    let s = ok_json(&f, &["session", "status", "--json"]).await?;
    assert_eq!(
        s,
        json!({
            "epoch": 1,
            "open": null,
            "recording": null,
            "stopped": null,
            "autosave": { "state": { "type": "off" }, "saved_at": null },
            "working": { "origin": null, "dirty": false, "saved_at": null, "recover": null },
        })
    );
    assert!(
        v["client_id"]
            .as_str()
            .is_some_and(|s| s.starts_with("local-"))
    );

    let same = FakeDaemon::start(FakeOptions {
        server: format!("ac2d 0.0.0 (build {})", ac2_cli::BUILD_ID),
        ..FakeOptions::default()
    })?;
    let v = ok_json(&same, &["daemon", "status", "--json"]).await?;
    assert_eq!(v["build_matches"], true);
    Ok(())
}
