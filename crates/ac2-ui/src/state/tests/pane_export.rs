//! Reducer tests of "Export pane as PNG…": the prompt's dated name and the export handed
//! to the app.

use super::*;
use crate::state::pane_export_name;

/// The name: the pane's measurement and view made a file name, local date and time.
#[test]
fn the_pane_export_name_is_dated_in_local_time() {
    // 2026-10-16 21:12:05 UTC is 00:12:05 the next day at UTC+3.
    let at = WallNs(1_792_185_125 * 1_000_000_000);
    assert_eq!(
        pane_export_name(Some("FOH L/R"), "Transfer", at, 3 * 3600),
        "FOH-L-R-Transfer-20261017-001205.png"
    );
    assert_eq!(
        pane_export_name(None, "Impulse response", at, 0),
        "Impulse-response-20261016-211205.png"
    );
    assert_eq!(
        pane_export_name(Some("  "), "Spectrum", at, 0),
        "Spectrum-20261016-211205.png"
    );
}

/// The palette's entry asks where, with `~/ac2-exports/<meas>-<view>-<date>.png`; Enter
/// hands the focused pane and the path to the app (no daemon request: the GPU is the
/// window's). A name without an extension gets `.png`.
#[test]
fn the_pane_export_prompt_hands_the_export_to_the_app() {
    let mut t = T::new();
    t.st.local_zone = crate::scenes::LocalZone::Fixed { offset_s: 0 };
    t.conn(mirror(with_spl()));
    let pane = t.st.layout.focus;
    let meas = t.st.pane_meas(pane).map(|m| m.config.name.clone());
    t.st.update(Msg::Command(CommandId::ExportPanePng), &t.keys);
    let Overlay::Prompt(p) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(p.kind, PromptKind::PaneExport(pane));
    let view = t.st.layout.view(pane).expect("view").title();
    let stem = match &meas {
        Some(m) => format!("~/ac2-exports/{}-{view}-20", m.replace(' ', "-")),
        None => format!("~/ac2-exports/{view}-20"),
    };
    assert!(p.text.starts_with(&stem), "{} vs {stem}", p.text);
    assert!(p.text.ends_with(".png"), "{}", p.text);
    if let Overlay::Prompt(p) = &mut t.st.overlay {
        p.text = "/tmp/report/foh".into();
    }
    let r = t.key("Enter");
    assert_eq!(t.st.overlay, Overlay::None);
    assert!(r.is_empty(), "{r:?}");
    assert_eq!(
        t.st.pane_export,
        Some(crate::state::PaneExport {
            pane,
            path: "/tmp/report/foh.png".into()
        })
    );
}
