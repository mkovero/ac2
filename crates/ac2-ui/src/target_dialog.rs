//! The built-in target curves' dialogs: a house tilt (slope, pivot, LF shelf) and a loudness
//! shift (two loudness levels). The text is read by `ac2_scene::target_curve`; the curve
//! itself is the daemon's to compute.

use ac2_proto::model::TargetCurve;
use ac2_scene::target_curve::{loudness, loudness_from_text, tilt, tilt_from_text};

use crate::forms::{Field, FieldId, Form, FormKind};

/// The house tilt dialog at its defaults (−1 dB/oct about 1 kHz, no shelf).
pub fn tilt_form() -> Form {
    Form::new(
        FormKind::TargetTilt,
        vec![
            Field::text(
                FieldId::Slope,
                tilt::SLOPE,
                tilt::DEFAULT_SLOPE,
                tilt::SLOPE_HINT,
            ),
            Field::text(
                FieldId::Pivot,
                tilt::PIVOT,
                tilt::DEFAULT_PIVOT,
                tilt::PIVOT_HINT,
            ),
            Field::text(
                FieldId::ShelfGain,
                tilt::SHELF_GAIN,
                tilt::DEFAULT_SHELF_GAIN,
                tilt::SHELF_GAIN_HINT,
            ),
            Field::text(
                FieldId::ShelfCorner,
                tilt::SHELF_CORNER,
                tilt::DEFAULT_SHELF_CORNER,
                tilt::SHELF_CORNER_HINT,
            ),
        ],
    )
}

/// The loudness shift dialog at its defaults (85 → 100 phon).
pub fn loudness_form() -> Form {
    Form::new(
        FormKind::TargetLoudness,
        vec![
            Field::text(
                FieldId::PhonFrom,
                loudness::FROM,
                loudness::DEFAULT_FROM,
                loudness::FROM_HINT,
            ),
            Field::text(
                FieldId::PhonTo,
                loudness::TO,
                loudness::DEFAULT_TO,
                loudness::TO_HINT,
            ),
        ],
    )
}

/// The curve the dialog asks for, or what is wrong with it.
pub fn curve(f: &Form) -> Result<TargetCurve, String> {
    match f.kind {
        FormKind::TargetTilt => tilt_from_text(
            f.text(FieldId::Slope),
            f.text(FieldId::Pivot),
            f.text(FieldId::ShelfGain),
            f.text(FieldId::ShelfCorner),
        ),
        FormKind::TargetLoudness => {
            loudness_from_text(f.text(FieldId::PhonFrom), f.text(FieldId::PhonTo))
        }
        _ => Err("not a target curve dialog".into()),
    }
}
