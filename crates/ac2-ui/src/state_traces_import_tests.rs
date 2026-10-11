//! Reducer tests of imports and exports: target curves, WAV impulse responses, the IR dialog.

use super::*;

#[test]
fn z_loads_a_target_curve_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let house = dir.path().join("house.txt");
    std::fs::write(&house, "20 0\n20000 0\n").expect("write");
    let mut t = T::new();
    t.type_key("Z", "z");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target) && p.text.is_empty()
    ));
    t.text(&house.display().to_string());
    let r = t.key("Enter");
    assert!(matches!(
        r.as_slice(),
        [Request::Import { path, role: ImportRole::Target, ir: None }] if *path == house
    ));
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn an_import_of_a_missing_file_says_so_in_the_prompt() {
    let mut t = T::new();
    let r = prompt_text(&mut t, CommandId::ImportTrace, "/nonexistent/house.txt");
    assert!(r.is_empty());
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.error.as_deref().is_some_and(|e| e.starts_with("cannot read"))
    ));
}

/// Imports a `channels`-channel 48 kHz WAV from the palette: the IR dialog opens.
fn ir_dialog(t: &mut T, channels: u16) -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let wav = dir.join("genelec.wav");
    let frames = vec![0.0f32; 4800 * usize::from(channels)];
    std::fs::write(
        &wav,
        ac2_traces::raw::wav::to_bytes(channels, 48_000, &frames),
    )
    .expect("write");
    let r = prompt_text(t, CommandId::ImportTrace, &wav.display().to_string());
    assert!(r.is_empty(), "a WAV asks its options first: {r:?}");
    wav
}

#[test]
fn a_wav_import_asks_its_channel_time_zero_gate_and_resolution() {
    use crate::forms::{FieldId, FormKind};
    let mut t = T::new();
    let wav = ir_dialog(&mut t, 2);
    let Overlay::Form(f) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(f.kind, FormKind::IrImport);
    let shown: Vec<(FieldId, String)> = f.fields.iter().map(|x| (x.id, x.display())).collect();
    assert_eq!(
        shown,
        [
            (FieldId::IrChannel, "1".to_string()),
            (
                FieldId::TimeZero,
                "IR peak (bulk delay removed)".to_string()
            ),
            (FieldId::Gate, "off".to_string()),
            (FieldId::GatePre, "1 ms".to_string()),
            (FieldId::Resolution, "1/48 oct".to_string()),
        ]
    );
    assert!(
        f.fields[0].hint.starts_with("2 channels, 48"),
        "{}",
        f.fields[0].hint
    );
    // The defaults are `ac2 trace import`'s, on channel 1.
    let r = t.key("Enter");
    assert!(
        matches!(
            r.as_slice(),
            [Request::Import { path, role: ImportRole::Trace, ir: Some(ir) }]
                if *path == wav && *ir == ac2_proto::model::IrImport { channel: Some(0), ..Default::default() }
        ),
        "{r:?}"
    );
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn the_ir_dialog_keeps_the_delay_gates_and_picks_a_channel() {
    use crate::forms::FieldId;
    use ac2_proto::model::{IrAlign, IrImport, IrImportWindow, Resolution};
    use ac2_proto::units::Seconds;
    let mut t = T::new();
    let wav = ir_dialog(&mut t, 2);
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!()
    };
    let at = |f: &crate::forms::Form, id| f.fields.iter().position(|x| x.id == id).expect("field");
    for (id, d) in [
        (FieldId::IrChannel, 1),
        (FieldId::TimeZero, 1),
        (FieldId::Resolution, 1),
    ] {
        let i = at(f, id);
        f.focus_field(i);
        f.cycle(d);
    }
    assert!(f.fields[at(f, FieldId::TimeZero)].hint.contains("latency"));
    f.set_text(FieldId::Gate, "10 ms");
    f.set_text(FieldId::GatePre, "0,5");
    let r = t.key("Enter");
    let want = IrImport {
        channel: Some(1),
        window: IrImportWindow::Gate {
            pre: Seconds(0.0005),
            post: Seconds(0.01),
        },
        align: IrAlign::Start,
        resolution: Resolution::NinetySixth,
    };
    assert!(
        matches!(
            r.as_slice(),
            [Request::Import { path, ir: Some(ir), .. }] if *path == wav && *ir == want
        ),
        "{r:?}"
    );
}

#[test]
fn a_bad_gate_keeps_the_ir_dialog_open() {
    use crate::forms::FieldId;
    let mut t = T::new();
    ir_dialog(&mut t, 1);
    let Overlay::Form(f) = &mut t.st.overlay else {
        panic!()
    };
    // A mono file has no channel to pick.
    assert!(f.fields.iter().all(|x| x.id != FieldId::IrChannel));
    f.set_text(FieldId::Gate, "soon");
    assert!(t.key("Enter").is_empty());
    let Overlay::Form(f) = &t.st.overlay else {
        panic!()
    };
    assert!(f.error.as_deref().is_some_and(|e| e.starts_with("gate:")));
}

#[test]
fn a_wav_export_path_asks_for_the_impulse_response() {
    use crate::conn::export_format;
    use ac2_proto::model::ExportFormat;
    use std::path::Path;
    assert_eq!(export_format(Path::new("/x/ir.wav")), ExportFormat::IrWav);
    assert_eq!(export_format(Path::new("/x/IR.WAV")), ExportFormat::IrWav);
    assert_eq!(export_format(Path::new("/x/ir.csv")), ExportFormat::Ac2Csv);
    assert_eq!(export_format(Path::new("/x/ir")), ExportFormat::Ac2Csv);
}

#[test]
fn stored_trace_metadata_follows_the_mirror() {
    let mut t = T::new();
    let meta = stored(9, Some(1), 2);
    t.conn(with_traces(vec![meta.clone()]));
    let data = TraceData {
        meta: meta.clone(),
        mag_db: vec![0.0; 3],
        phase_deg: None,
        coherence: None,
        sweep: None,
        ir: None,
    };
    t.conn(ConnEvent::Trace(
        Arc::new(data),
        Arc::new(ac2_proto::GridDef::Log {
            ppo: 1,
            k_min: 0,
            k_max: 2,
        }),
    ));
    let mut hidden = meta.clone();
    hidden.edit.visible = false;
    hidden.edit.slot = None;
    t.conn(with_traces(vec![hidden.clone()]));
    assert_eq!(t.st.traces[&TraceId(9)].0.meta, hidden);
    t.conn(with_traces(vec![]));
    assert!(t.st.traces.is_empty());
}
