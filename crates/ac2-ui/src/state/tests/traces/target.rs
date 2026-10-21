//! Reducer tests of the built-in target curves: the Z chooser, its dialogs and the
//! `trace.target` each sends.

use super::*;
use crate::forms::{FieldId, FormKind};
use ac2_proto::model::{LfShelf, TargetCurve};
use ac2_proto::units::{Db, Hz, Phon};

/// Z, then `downs` rows down and Enter.
fn pick(t: &mut T, downs: usize) -> Vec<Request> {
    t.type_key("Z", "z");
    for _ in 0..downs {
        t.key("Down");
    }
    t.key("Enter")
}

/// The one `trace.target` in `r`: its curve, name and toast text.
fn target(r: &[Request]) -> (&TargetCurve, &str, &str) {
    match r {
        [
            Request::Call {
                cmd: Command::TraceTarget { curve, name },
                what,
            },
        ] => (curve, name, what),
        r => panic!("{r:?}"),
    }
}

fn form(t: &mut T) -> &mut crate::forms::Form {
    match &mut t.st.overlay {
        Overlay::Form(f) => f,
        o => panic!("{o:?}"),
    }
}

#[test]
fn z_offers_a_file_and_the_built_in_curves() {
    let mut t = T::new();
    t.type_key("Z", "z");
    let Overlay::Choose(c) = &t.st.overlay else {
        panic!("{:?}", t.st.overlay)
    };
    assert_eq!(c.purpose, ChoicePurpose::Target);
    assert_eq!(c.title, "Target curve");
    let rows: Vec<&str> = c.choices.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(rows.len(), 4);
    for (row, start) in rows
        .iter()
        .zip(["Load a file…", "Flat", "House tilt…", "Loudness shift…"])
    {
        assert!(row.starts_with(start), "{row}");
    }
    t.key("Esc");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn flat_is_added_at_once() {
    let mut t = T::new();
    let r = pick(&mut t, 1);
    let (curve, name, what) = target(&r);
    assert_eq!(*curve, TargetCurve::Flat);
    assert_eq!(name, "flat");
    assert_eq!(what, "target flat added");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn the_tilt_dialog_sends_its_curve_named_by_its_parameters() {
    let mut t = T::new();
    assert!(pick(&mut t, 2).is_empty());
    let f = form(&mut t);
    assert_eq!(f.kind, FormKind::TargetTilt);
    let shown: Vec<(&str, String)> = f
        .fields
        .iter()
        .map(|x| (x.label.as_str(), x.display()))
        .collect();
    assert_eq!(
        shown,
        [
            ("Slope", "−1.0 dB/oct".to_owned()),
            ("Pivot", "1 kHz".to_owned()),
            ("LF shelf", "off".to_owned()),
            ("Shelf corner", "100 Hz".to_owned()),
        ]
    );
    // A slope past the limit stays in the dialog with the reason.
    f.set_text(FieldId::Slope, "-9");
    assert!(t.key("Enter").is_empty());
    let e = form(&mut t).error.clone().expect("an error");
    assert!(e.contains("slope"), "{e}");
    let f = form(&mut t);
    f.set_text(FieldId::Slope, "-1");
    f.set_text(FieldId::ShelfGain, "6");
    let r = t.key("Enter");
    let (curve, name, _) = target(&r);
    assert_eq!(
        *curve,
        TargetCurve::Tilt {
            slope_db_per_oct: -1.0,
            pivot: Hz(1000.0),
            lf_shelf: Some(LfShelf {
                gain: Db(6.0),
                corner: Hz(100.0)
            }),
        }
    );
    assert_eq!(name, "tilt −1.0 dB/oct @ 1 kHz, LF +6.0 dB @ 100 Hz");
    assert_eq!(t.st.overlay, Overlay::None);
}

#[test]
fn the_loudness_dialog_says_it_is_not_an_eq_target() {
    let mut t = T::new();
    assert!(pick(&mut t, 3).is_empty());
    let f = form(&mut t);
    assert_eq!(f.kind, FormKind::TargetLoudness);
    assert_eq!(f.kind.title(), "Loudness shift (not an EQ target)");
    f.set_text(FieldId::PhonTo, "110");
    assert!(t.key("Enter").is_empty());
    let e = form(&mut t).error.clone().expect("an error");
    assert!(e.contains("20 … 100 phon"), "{e}");
    form(&mut t).set_text(FieldId::PhonTo, "100");
    let r = t.key("Enter");
    let (curve, name, _) = target(&r);
    assert_eq!(
        *curve,
        TargetCurve::LoudnessShift {
            from: Phon(85.0),
            to: Phon(100.0)
        }
    );
    assert_eq!(name, "loudness 85 → 100 phon");
}

#[test]
fn the_palette_reaches_each_curve_without_the_chooser() {
    let mut t = T::new();
    let r = t.st.update(Msg::Command(CommandId::TargetFlat), &t.keys);
    assert_eq!(*target(&r).0, TargetCurve::Flat);
    t.st.update(Msg::Command(CommandId::TargetTilt), &t.keys);
    assert_eq!(form(&mut t).kind, FormKind::TargetTilt);
    t.key("Esc");
    t.st.update(Msg::Command(CommandId::TargetLoudness), &t.keys);
    assert_eq!(form(&mut t).kind, FormKind::TargetLoudness);
    t.key("Esc");
    t.st.update(Msg::Command(CommandId::TargetFile), &t.keys);
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::ImportFile(ImportRole::Target)
    ));
}
