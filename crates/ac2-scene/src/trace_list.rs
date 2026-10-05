//! The list of stored traces beside the panes: every trace by name, what it is, its slot,
//! whether it is shown, and its colour, in the order the selection keys step through.

use ac2_proto::model::{MathOp, TraceKind, TraceMeta, TraceSource};
use ac2_proto::units::TraceId;

use crate::format;
use crate::primitives::Color;

/// What a trace is, as the list names it: `sweep`, `capture`, `imported`, `average`,
/// `A − B`, `target` …
pub fn kind_name(t: &TraceMeta) -> &'static str {
    match (t.kind, &t.source) {
        (TraceKind::Target, _) => "target",
        (_, TraceSource::IrCapture { .. }) => "sweep",
        (TraceKind::Sweep, TraceSource::Imported { .. }) => "imported sweep",
        (_, TraceSource::Imported { .. }) => "imported",
        (_, TraceSource::Average { .. }) => "average",
        (_, TraceSource::SpatialAverage { .. }) => "spatial average",
        (
            _,
            TraceSource::Math {
                op: MathOp::MagnitudeDifference,
                ..
            },
        ) => "A − B",
        (
            _,
            TraceSource::Math {
                op: MathOp::ComplexDivision,
                ..
            },
        ) => "A / B",
        (TraceKind::Spectrum { .. }, TraceSource::Captured { .. }) => "spectrum capture",
        (TraceKind::Rta { .. }, TraceSource::Captured { .. }) => "RTA capture",
        (TraceKind::Sweep, TraceSource::Captured { .. }) => "sweep",
        (TraceKind::Transfer, TraceSource::Captured { .. }) => "capture",
    }
}

/// The list's order, which the selection keys follow too: slotted traces by slot, then the
/// rest in display order (oldest first).
pub fn sort_key(t: &TraceMeta) -> (u8, u32, TraceId) {
    (t.edit.slot.unwrap_or(u8::MAX), t.edit.order, t.id)
}

/// One stored trace as the list gets it.
#[derive(Clone, Copy, Debug)]
pub struct TraceItem<'a> {
    pub meta: &'a TraceMeta,
    /// Its columns have arrived (a trace without them is listed but draws nothing yet).
    pub has_data: bool,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceRow {
    pub id: TraceId,
    /// The trace's name, as given.
    pub name: String,
    /// [`kind_name`].
    pub kind: &'static str,
    pub slot: Option<u8>,
    pub shown: bool,
    pub selected: bool,
    /// The colour its curve is drawn in.
    pub color: Color,
    /// The line under the name, longest first: the list shows the first that fits its
    /// width. Every variant starts with the kind; the shortest is the kind alone.
    pub details: Vec<String>,
    /// Everything about the row in one sentence (the tooltip, and what a screen reader says).
    pub describe: String,
}

/// The smoothing a row names: `1/6 oct`, `1/6 oct mag only`; a spectrum has no phase, so
/// its smoothing has no mode to name.
fn smoothing_text(t: &TraceMeta) -> Option<String> {
    let s = t.edit.smoothing?;
    Some(match t.kind {
        TraceKind::Spectrum { .. } => format!("smoothed {}", format::octave_fraction(s.fraction)),
        _ => format::smoothing(Some(s)),
    })
}

/// The rows of the list, in [`sort_key`] order.
pub fn trace_rows(items: &[TraceItem<'_>], selected: Option<TraceId>) -> Vec<TraceRow> {
    let mut items: Vec<&TraceItem<'_>> = items.iter().collect();
    items.sort_by_key(|i| sort_key(i.meta));
    items
        .into_iter()
        .map(|i| {
            let t = i.meta;
            let kind = kind_name(t);
            let slot = t.edit.slot.map(|n| format!("slot {n}"));
            let hidden = (!t.edit.visible).then(|| "hidden".to_owned());
            let locked = t.edit.locked.then(|| "locked".to_owned());
            let no_data = (!i.has_data).then(|| "no data yet".to_owned());
            let smooth = smoothing_text(t);
            // Dropped from the end first: what the eye and the colour already say last.
            let parts = |extra: bool| -> String {
                let mut v = vec![kind.to_owned()];
                v.extend(slot.clone());
                v.extend(hidden.clone());
                if extra {
                    v.extend(no_data.clone());
                    v.extend(locked.clone());
                    v.extend(smooth.clone());
                }
                v.join(" · ")
            };
            let mut details = vec![parts(true), parts(false)];
            if slot.is_some() && hidden.is_some() {
                details.push(format!("{kind} · {}", slot.clone().unwrap_or_default()));
            }
            details.push(kind.to_owned());
            details.dedup();
            let c = t.edit.color;
            let describe = format!(
                "{}: {}, {}",
                t.edit.name,
                parts(true),
                if t.edit.visible { "shown" } else { "hidden" }
            );
            TraceRow {
                id: t.id,
                name: t.edit.name.clone(),
                kind,
                slot: t.edit.slot,
                shown: t.edit.visible,
                selected: selected == Some(t.id),
                color: Color::from_rgba8([c.r, c.g, c.b, 255]),
                details,
                describe,
            }
        })
        .collect()
}

/// The confirmation before a stored trace is deleted: which one, what goes with it, and the
/// keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteConfirm {
    /// `Delete Main L S1?`
    pub title: String,
    /// What it is (`capture · slot 1`), then what deleting means.
    pub lines: Vec<String>,
    pub hint: String,
}

/// What the confirmation says for `row`.
pub fn delete_confirm(row: &TraceRow) -> DeleteConfirm {
    DeleteConfirm {
        title: format!("Delete {}?", row.name),
        lines: vec![
            row.details.first().cloned().unwrap_or_default(),
            "The stored trace is removed from the daemon (and from sessions saved after this); \
             it cannot be undone."
                .to_owned(),
        ],
        hint: "Delete or Enter deletes it · N or Esc keeps it (Esc also stops the stimulus)"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::model::*;
    use ac2_proto::units::*;

    fn meta(
        id: u32,
        name: &str,
        slot: Option<u8>,
        kind: TraceKind,
        source: TraceSource,
    ) -> TraceMeta {
        TraceMeta {
            id: TraceId(id),
            edit: TraceEdit {
                name: name.into(),
                color: Rgb {
                    r: 10,
                    g: 20,
                    b: 30,
                },
                visible: true,
                locked: false,
                order: id,
                offset: Db(0.0),
                polarity: Polarity::Normal,
                delay_nudge: Seconds(0.0),
                slot,
                smoothing: None,
            },
            kind,
            source,
            grid_id: ac2_proto::GridId(1),
            delay: Seconds(0.0),
            depth: None,
            cal: CalState::Uncalibrated,
            mic: None,
            mic_curve: None,
            created_at: WallNs(0),
        }
    }

    fn captured() -> TraceSource {
        TraceSource::Captured {
            meas: MeasId(1),
            meas_name: "Main L".into(),
            epoch: SessionEpoch(1),
            at_sample: SampleIndex(0),
        }
    }

    fn swept() -> TraceSource {
        TraceSource::IrCapture {
            run: SweepId(1),
            epoch: SessionEpoch(1),
            sweep: EssSpec {
                start: Hz(20.0),
                end: Hz(20_000.0),
                duration: Seconds(1.0),
                fade_in: Seconds(0.0),
                fade_out: Seconds(0.0),
            },
            level: Dbfs(-20.0),
            repeats: 1,
            reference_input: 0,
            measurement_input: 1,
        }
    }

    fn imported() -> TraceSource {
        TraceSource::Imported {
            file_name: "house.txt".into(),
            format: ImportFormat::AnalyzerText,
            notes: vec![],
        }
    }

    #[test]
    fn every_kind_is_named() {
        let cases = [
            (TraceKind::Transfer, captured(), "capture"),
            (TraceKind::Sweep, swept(), "sweep"),
            (TraceKind::Transfer, imported(), "imported"),
            (TraceKind::Sweep, imported(), "imported sweep"),
            (TraceKind::Target, imported(), "target"),
            (
                TraceKind::Spectrum {
                    scale: LevelScale::Dbfs,
                },
                captured(),
                "spectrum capture",
            ),
            (
                TraceKind::Rta {
                    scale: LevelScale::Dbfs,
                },
                captured(),
                "RTA capture",
            ),
            (
                TraceKind::Transfer,
                TraceSource::Average {
                    traces: vec![],
                    method: AverageMethod::Power,
                    reference: DelayReference::Fixed {
                        delay: Seconds(0.0),
                    },
                },
                "average",
            ),
            (
                TraceKind::Transfer,
                TraceSource::Math {
                    a: TraceId(1),
                    b: TraceId(2),
                    op: MathOp::MagnitudeDifference,
                },
                "A − B",
            ),
            (
                TraceKind::Transfer,
                TraceSource::Math {
                    a: TraceId(1),
                    b: TraceId(2),
                    op: MathOp::ComplexDivision,
                },
                "A / B",
            ),
        ];
        for (kind, source, want) in cases {
            assert_eq!(kind_name(&meta(1, "x", None, kind, source)), want);
        }
    }

    #[test]
    fn rows_list_slotted_first_then_by_order_and_name_each_trace() {
        let sweep_b = meta(7, "Sweep 2", None, TraceKind::Sweep, swept());
        let sweep_a = meta(5, "Sweep 1", None, TraceKind::Sweep, swept());
        let mut slot3 = meta(9, "Main L S3", Some(3), TraceKind::Transfer, captured());
        slot3.edit.visible = false;
        slot3.edit.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::MagnitudePhase,
        });
        let target = meta(2, "house", None, TraceKind::Target, imported());
        let items = [
            TraceItem {
                meta: &sweep_b,
                has_data: true,
            },
            TraceItem {
                meta: &slot3,
                has_data: true,
            },
            TraceItem {
                meta: &sweep_a,
                has_data: false,
            },
            TraceItem {
                meta: &target,
                has_data: true,
            },
        ];
        let rows = trace_rows(&items, Some(TraceId(7)));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Main L S3", "house", "Sweep 1", "Sweep 2"]);
        let selected: Vec<bool> = rows.iter().map(|r| r.selected).collect();
        assert_eq!(selected, [false, false, false, true]);
        assert_eq!(
            rows[0].details,
            [
                "capture · slot 3 · hidden · 1/6 oct",
                "capture · slot 3 · hidden",
                "capture · slot 3",
                "capture",
            ]
        );
        assert!(!rows[0].shown);
        assert_eq!(
            rows[0].describe,
            "Main L S3: capture · slot 3 · hidden · 1/6 oct, hidden"
        );
        assert_eq!(rows[1].details, ["target"]);
        assert_eq!(rows[2].details, ["sweep · no data yet", "sweep"]);
        assert_eq!(rows[3].details, ["sweep"]);
        assert_eq!(rows[3].describe, "Sweep 2: sweep, shown");
        assert_eq!(rows[3].color, Color::from_rgba8([10, 20, 30, 255]));
    }

    #[test]
    fn details_shorten_towards_the_kind() {
        let mut t = meta(4, "FOH", Some(1), TraceKind::Transfer, captured());
        t.edit.locked = true;
        t.edit.smoothing = Some(Smoothing {
            fraction: SmoothingFraction::Third,
            mode: SmoothingMode::Magnitude,
        });
        let rows = trace_rows(
            &[TraceItem {
                meta: &t,
                has_data: true,
            }],
            None,
        );
        let d = &rows[0].details;
        assert_eq!(d[0], "capture · slot 1 · locked · 1/3 oct mag only");
        assert_eq!(d.last().map(String::as_str), Some("capture"));
        for w in d.windows(2) {
            assert!(w[0].chars().count() > w[1].chars().count(), "{d:?}");
        }
    }
}
