//! The generator: refusals and the output ceiling.

use super::{R, ac2, fake, ok_json};

#[tokio::test(flavor = "multi_thread")]
async fn gen_refusals_never_touch_the_lease() -> R {
    let f = fake()?;
    // The fake daemon's ceiling is −6 dBFS.
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
    assert_eq!(f.executions("gen.acquire"), 0);

    let v = ok_json(&f, &["gen", "stop", "--json"]).await?;
    assert_eq!(v["stopped"], true);
    assert_eq!(f.executions("gen.stop"), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn gen_ceiling_shows_lowers_and_raises_only_with_yes() -> R {
    let f = fake()?;
    // The fake daemon's ceiling and bound are −6 dBFS.
    let v = ok_json(&f, &["gen", "ceiling", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["bound"].as_f64()),
        (Some(-6.0), Some(-6.0))
    );
    let v = ok_json(&f, &["gen", "ceiling", "-40dbfs", "--json"]).await?;
    assert_eq!(
        (v["ceiling"].as_f64(), v["previous"].as_f64()),
        (Some(-40.0), Some(-6.0))
    );
    let r = ac2(&f, &["gen", "ceiling", "-30dbfs"]).await?;
    assert_eq!(
        r.code, 4,
        "a raise without --yes is refused before anything is sent"
    );
    assert_eq!(f.executions("gen.ceiling"), 1);
    let v = ok_json(&f, &["gen", "ceiling", "-30dbfs", "--yes", "--json"]).await?;
    assert_eq!(v["ceiling"].as_f64(), Some(-30.0));
    // Above the bound: the daemon refuses it whatever the confirmation.
    let r = ac2(&f, &["gen", "ceiling", "-3dbfs", "--yes", "--json"]).await?;
    assert_ne!(r.code, 0);
    let r = ac2(&f, &["gen", "ceiling"]).await?;
    assert_eq!(r.code, 0);
    assert!(
        r.stdout
            .contains("system max level \u{2212}30.0 dBFS · bound \u{2212}6.0 dBFS")
            && r.stdout.contains("(raised by "),
        "{}",
        r.stdout
    );
    Ok(())
}
