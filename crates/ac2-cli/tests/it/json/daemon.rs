//! Devices and daemon status.

use serde_json::{Value, json};

use super::{R, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn devices_json() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["devices", "--json"]).await?;
    let dir = |ch, spare, names: serde_json::Value| {
        json!({
            "max_channels": ch,
            "rates_hz": [{ "min": 48000, "max": 48000 }],
            "buffer_frames": { "min": 256, "max": 256 },
            "default_rate_hz": 48000,
            "default_buffer_frames": 256,
            "channel_names": names,
            "spare_channels": spare,
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
                    "id": "fake",
                    "name": "simulated duplex device",
                    "input": dir(4, 8, json!(["Loop return", "Room mic", "Hall mic", "Line 4"])),
                    "output": dir(2, 0, json!(["Out 1 (speaker + loop)", "Out 2"])),
                    "duplex_clock": "single_callback",
                    "index": "exact",
                    "clock_sources": [],
                    "notes": []
                }]
            }
        ])
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_build_ids() -> R {
    let f = fake()?;
    let v = ok_json(&f, &["status", "--json"]).await?;
    // The daemon in this process is built from the same tree as the CLI.
    let id = format!(
        "ac2d {} (build {})",
        env!("CARGO_PKG_VERSION"),
        ac2_cli::BUILD_ID
    );
    assert_eq!(v["server"], id);
    assert_eq!(v["build_id"], ac2_cli::BUILD_ID);
    assert_eq!(v["client_build_id"], ac2_cli::BUILD_ID);
    assert_eq!(v["build_matches"], true);
    // Not a local daemon: "stale" is not judged.
    assert_eq!(v["stale"], Value::Null);
    assert!(
        v["daemon_incarnation"]
            .as_str()
            .is_some_and(|s| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit())),
        "{v}"
    );
    assert_eq!(
        v["session"],
        json!({ "epoch": 0, "open": null, "stopped": null })
    );
    assert_eq!(
        v["autosave"],
        json!({ "state": { "type": "off" }, "saved_at": null, "log_failures": [] })
    );
    let s = ok_json(&f, &["session", "status", "--json"]).await?;
    assert_eq!(
        s,
        json!({
            "epoch": 0,
            "open": null,
            "recording": null,
            "stopped": null,
            "autosave": { "state": { "type": "off" }, "saved_at": null, "log_failures": [] },
            "working": { "origin": null, "dirty": false, "saved_at": null, "recover": null },
        })
    );
    assert!(
        v["client_id"]
            .as_str()
            .is_some_and(|s| s.starts_with("local-"))
    );
    let d = ok_json(&f, &["daemon", "status", "--json"]).await?;
    assert_eq!(d["build_matches"], true);
    Ok(())
}
