//! "Export pane as PNG…" end to end: the palette, the prompt, the window's GPU painting
//! the pane's scene offscreen and the PNG on disk.

use super::*;

/// The focused transfer pane written where the prompt says: twice the plot's on-screen
/// pixels (the harness draws at 1 px per point), opaque, and a picture rather than a
/// blank plate; the toast names the file and its size.
#[test]
fn the_focused_pane_exports_as_png() {
    if !have_gpu("the_focused_pane_exports_as_png") {
        return;
    }
    let rig = common::Rig::start();
    let mut h = harness(options(Some(&rig)));
    step_until(&mut h, "live frames", live);
    settle(&mut h, 3);
    let pane = h.state().state.layout.focus;
    assert_eq!(h.state().state.layout.kind(pane), PaneKind::Transfer);
    let size = h
        .state()
        .pane_plot_size(pane)
        .expect("the pane drew a plot");

    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text("export pane".into()));
    step_until(&mut h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(&mut h, "export prompt", |a| {
        matches!(a.state.overlay, Overlay::Prompt(_))
    });
    let Overlay::Prompt(p) = &h.state().state.overlay else {
        unreachable!()
    };
    assert!(p.text.starts_with("~/ac2-exports/"), "{}", p.text);
    assert!(p.text.ends_with(".png"), "{}", p.text);

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("report").join("transfer.png");
    if let Overlay::Prompt(p) = &mut h.state_mut().state.overlay {
        p.text = path.display().to_string();
    }
    h.key_press(Key::Enter);
    step_until(&mut h, "export toast", |a| {
        a.state
            .toasts
            .iter()
            .any(|t| t.text.contains("exported to"))
    });
    let toast = h
        .state()
        .state
        .toasts
        .iter()
        .find(|t| t.text.contains("exported to"))
        .map(|t| t.text.clone())
        .expect("toast");
    let want = [
        (size[0] * 2.0).round() as u32,
        (size[1] * 2.0).round() as u32,
    ];
    assert_eq!(
        toast,
        format!(
            "Transfer pane exported to {} ({}×{} px)",
            path.display(),
            want[0],
            want[1]
        )
    );

    let file = std::fs::File::open(&path).expect("the PNG written");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("decoded");
    assert_eq!([info.width, info.height], want);
    assert_eq!(info.color_type, png::ColorType::Rgba);
    let px = &buf[..info.buffer_size()];
    assert!(px.chunks(4).all(|p| p[3] == 255), "opaque");
    let colours: std::collections::HashSet<&[u8]> = px.chunks(4).collect();
    assert!(
        colours.len() > 50,
        "curves, grid and text drawn: {} colours",
        colours.len()
    );
}
