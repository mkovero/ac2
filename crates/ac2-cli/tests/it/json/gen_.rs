//! The generator: refusals and the output ceiling.

use serde_json::Value;

use super::{R, ac2, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn gen_refusals_never_touch_the_lease() -> R {
    let f = fake()?;
    // The daemon's ceiling is −10 dBFS.
    let r = ac2(
        &f,
        &["gen", "pink", "--out", "1", "--level", "-3dbfs", "--json"],
    )
    .await?;
    assert_eq!(r.code, 4);
    assert_eq!(r.json()?["error"]["code"], "refused");
    // No session open.
    let r = ac2(
        &f,
        &["gen", "pink", "--out", "1", "--level", "-30dbfs", "--json"],
    )
    .await?;
    assert_eq!(r.code, 1);
    assert_eq!(r.json()?["error"]["code"], "usage");
    // Band limits on a sine.
    let r = ac2(
        &f,
        &[
            "gen", "sine", "--freq", "1khz", "--hp", "30hz", "--out", "1", "--level", "-30dbfs",
        ],
    )
    .await?;
    assert_eq!(r.code, 1);
    // Nobody took the generator.
    let dump = ok_json(&f, &["state", "dump"]).await?;
    assert_eq!(dump["state"]["generator"]["owner"], Value::Null, "{dump}");
    assert_eq!(dump["state"]["generator"]["armed"], false);

    let v = ok_json(&f, &["gen", "stop", "--json"]).await?;
    assert_eq!(v["stopped"], true);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gen_ceiling_shows_lowers_and_raises_only_with_yes() -> R {
    let f = fake()?;
    // The daemon's ceiling and bound are −10 dBFS.
    let v = ok_json(&f, &["gen", "ceiling", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["bound"].as_f64()),
        (Some(-10.0), Some(-10.0))
    );
    let v = ok_json(&f, &["gen", "ceiling", "-40dbfs", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["previous"].as_f64()),
        (Some(-40.0), Some(-10.0))
    );
    let r = ac2(&f, &["gen", "ceiling", "-30dbfs"]).await?;
    assert_eq!(
        r.code, 4,
        "a raise without --yes is refused before anything is sent"
    );
    let v = ok_json(&f, &["gen", "ceiling", "--json"]).await?;
    assert_eq!(v["ceiling"].as_f64(), Some(-40.0), "nothing was sent");
    let v = ok_json(&f, &["gen", "ceiling", "-30dbfs", "--yes", "--json"]).await?;
    assert_eq!(v["ceiling"].as_f64(), Some(-30.0));
    // Above the bound: the daemon refuses it whatever the confirmation.
    let r = ac2(&f, &["gen", "ceiling", "-3dbfs", "--yes", "--json"]).await?;
    assert_ne!(r.code, 0);
    let r = ac2(&f, &["gen", "ceiling"]).await?;
    assert_eq!(r.code, 0);
    assert!(
        r.stdout
            .contains("system max level \u{2212}30.0 dBFS · bound \u{2212}10.0 dBFS")
            && r.stdout.contains("(raised by "),
        "{}",
        r.stdout
    );
    Ok(())
}
