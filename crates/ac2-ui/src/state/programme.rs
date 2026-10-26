//! Programme panes: what each meters, the app-side flying peaks and goniometer rings fed
//! from the session's levels and gonio frames, the loudness column's source, and the reset of
//! max, overs and loudness.
//!
//! The daemon computes every number (ballistics, true peak, max, overs, correlation,
//! loudness, the gonio points); the app only holds what is a matter of display: the flying
//! peak above a bar and the embers' ages.

use std::collections::BTreeMap;

use ac2_proto::FrameData;
use ac2_proto::model::layout::PaneContent;
use ac2_proto::topic::Topic;
use ac2_scene::gonio::EmberRing;
use ac2_scene::programme::loudness::{LoudnessFacts, NamedPair, plate_readout};
use ac2_scene::programme::{
    Ballistic, Correlation, FlyingPeak, ProgrammeInput, ProgrammePair, ProgrammePane, Scale, Side,
    bars_of, peak_bars, sources,
};

use crate::conn::{DataSnapshot, Request};
use crate::keys::{CommandId, Scope};
use crate::state::{AppState, PaneId, PaneKind};

/// Bars a Programme pane shows at most (the wire's limit).
pub const MAX_METERED: usize = PaneContent::MAX_INPUTS;

/// Embers a pair's ring holds: 256 points a frame for about a second of frames.
const RING_CAPACITY: usize = ac2_proto::frame::GONIO_POINTS * 32;

/// What a Programme pane meters: up to [`MAX_METERED`] inputs side by side, or a stereo
/// pair. Zero-based device inputs, as the session names them. A fixed array, so a pane's
/// view stays a plain value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Metered {
    Inputs { n: u8, inputs: [u16; MAX_METERED] },
    Pair { left: u16, right: u16 },
}

impl Metered {
    /// One to [`MAX_METERED`] distinct inputs; `None` otherwise.
    pub fn inputs_of(list: &[u16]) -> Option<Self> {
        let content = PaneContent::Inputs {
            inputs: list.to_vec(),
        };
        content.check().ok()?;
        let mut inputs = [0; MAX_METERED];
        inputs[..list.len()].copy_from_slice(list);
        Some(Self::Inputs {
            n: list.len() as u8,
            inputs,
        })
    }

    /// The inputs metered, left to right.
    pub fn inputs(&self) -> Vec<u16> {
        match self {
            Self::Inputs { n, inputs } => inputs[..usize::from(*n)].to_vec(),
            Self::Pair { left, right } => vec![*left, *right],
        }
    }

    pub fn pair(&self) -> Option<(u16, u16)> {
        match self {
            Self::Pair { left, right } => Some((*left, *right)),
            Self::Inputs { .. } => None,
        }
    }

    pub fn wire(&self) -> PaneContent {
        match self {
            Self::Inputs { .. } => PaneContent::Inputs {
                inputs: self.inputs(),
            },
            Self::Pair { left, right } => PaneContent::Pair {
                left: *left,
                right: *right,
            },
        }
    }

    /// The meters a layout's content names; `None` for a measurement or content the wire
    /// check refuses.
    pub fn from_wire(c: &PaneContent) -> Option<Self> {
        c.check().ok()?;
        match c {
            PaneContent::Meas { .. } => None,
            PaneContent::Inputs { inputs } => Self::inputs_of(inputs),
            PaneContent::Pair { left, right } => Some(Self::Pair {
                left: *left,
                right: *right,
            }),
        }
    }
}

/// What the app keeps for the Programme panes between frames.
#[derive(Clone, Debug, Default)]
pub struct ProgrammeUi {
    /// The flying peak per input, fed from every levels frame.
    pub flying: BTreeMap<u16, FlyingPeak>,
    /// The embers of each pair a visible pane shows.
    pub rings: BTreeMap<(u16, u16), EmberRing>,
    /// The last levels and gonio frames folded (their sequence numbers).
    levels_seq: Option<u64>,
    gonio_seq: Option<u64>,
}

impl ProgrammeUi {
    /// Whether an ember is still cooling somewhere: the panes then redraw every frame.
    pub fn glowing(&self) -> bool {
        self.rings.values().any(|r| !r.is_empty())
    }
}

/// The scene's ballistic for the wire's.
pub fn ballistic(b: ac2_proto::model::Ballistic) -> Ballistic {
    use ac2_proto::model::Ballistic as W;
    match b {
        W::TruePeak => Ballistic::TruePeak,
        W::Type1 => Ballistic::Type1,
        W::Type2 => Ballistic::Type2,
        W::Vu => Ballistic::Vu,
        W::Momentary => Ballistic::Momentary,
        W::ShortTerm => Ballistic::ShortTerm,
    }
}

/// A pair's correlation as its levels frame carries it.
pub fn correlation(p: Option<&ac2_proto::frame::PairLevels>) -> Correlation {
    use ac2_proto::model::PairSide;
    match p {
        None => Correlation::NoData,
        Some(p) => match (p.correlation, p.quiet) {
            (_, Some(PairSide::Left)) => Correlation::Quiet(Side::Left),
            (_, Some(PairSide::Right)) => Correlation::Quiet(Side::Right),
            (Some(r), None) => Correlation::Value(r),
            (None, None) => Correlation::NoData,
        },
    }
}

impl AppState {
    /// The name in pane `id`'s title: a Programme pane says what it meters (`true peak ·
    /// Main L + Main R · 1 s`), the other views their kind.
    pub fn pane_title(&self, id: PaneId) -> String {
        let Some(view) = self.layout.view(id) else {
            return self.layout.kind(id).title().to_owned();
        };
        if view.kind == PaneKind::Programme
            && let Some((p, missing)) = self.programme_pane(id)
            && missing.is_empty()
        {
            return p.title();
        }
        view.title().to_owned()
    }

    /// Whether a visible goniometer has embers still cooling: it redraws every frame.
    pub fn embers_glowing(&self) -> bool {
        self.programme.glowing()
    }

    /// The pairs the visible Programme panes show.
    pub fn programme_pairs_shown(&self) -> Vec<(u16, u16)> {
        let mut v: Vec<(u16, u16)> = self
            .visible_panes()
            .into_iter()
            .filter_map(|id| self.layout.view(id))
            .filter(|v| v.kind == PaneKind::Programme)
            .filter_map(|v| v.meters.and_then(|m| m.pair()))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Feeds the flying peaks from a new levels frame and the shown pairs' rings from a new
    /// gonio frame, at display time `now_s`; drops the rings no visible pane shows.
    pub(in crate::state) fn fold_programme(&mut self, d: &DataSnapshot) {
        let now = self.now_s;
        if let Some(tf) = d.latest.get(&Topic::SessionLevels)
            && let FrameData::SessionLevels(f) = &tf.frame.data
            && self.programme.levels_seq != Some(tf.frame.stamp.seq)
        {
            self.programme.levels_seq = Some(tf.frame.stamp.seq);
            for (i, c) in f.meta.channels.iter().enumerate() {
                let ppm = f.ppm.get(i).copied();
                self.programme
                    .flying
                    .entry(*c)
                    .or_default()
                    .update(now, ppm);
            }
        }
        let shown = self.programme_pairs_shown();
        self.programme.rings.retain(|k, _| shown.contains(k));
        if let Some(tf) = d.latest.get(&Topic::SessionGonio)
            && let FrameData::SessionGonio(g) = &tf.frame.data
            && self.programme.gonio_seq != Some(tf.frame.stamp.seq)
        {
            self.programme.gonio_seq = Some(tf.frame.stamp.seq);
            for p in &g.pairs {
                if shown.contains(&(p.left, p.right)) {
                    self.programme
                        .rings
                        .entry((p.left, p.right))
                        .or_insert_with(|| EmberRing::new(RING_CAPACITY))
                        .push_points(now, &p.points);
                }
            }
        }
    }

    /// Lets the embers older than the decay go (every frame tick).
    pub(in crate::state) fn retire_embers(&mut self) {
        let (now, decay) = (self.now_s, self.ember_decay_s());
        for r in self.programme.rings.values_mut() {
            r.retire(now, decay);
        }
    }

    /// The ember cooling time constant the operator set (Settings › Display).
    pub fn ember_decay_s(&self) -> f32 {
        ac2_scene::gonio::clamp_decay(self.prefs.ember_decay_s)
    }

    /// What Programme pane `id` shows: its bars, its pair's chip, the ballistic in force;
    /// with the inputs it names that the open session lacks (all of them with no session).
    pub fn programme_pane(&self, id: PaneId) -> Option<(ProgrammePane, Vec<u16>)> {
        let v = self
            .layout
            .view(id)
            .filter(|v| v.kind == PaneKind::Programme)?;
        let metered = v.meters?;
        let captured = self
            .open_session()
            .map(|o| o.config.input_channels.clone())
            .unwrap_or_default();
        let missing: Vec<u16> = metered
            .inputs()
            .into_iter()
            .filter(|c| !captured.contains(c))
            .collect();
        Some((self.programme_of(metered), missing))
    }

    /// What `metered` reads in the latest levels frame: each input's bar and readouts, the
    /// pair's correlation. The Programme panes and the Inputs list read the same.
    pub fn programme_of(&self, metered: Metered) -> ProgrammePane {
        let st = self.daemon();
        let latest = self
            .data
            .as_ref()
            .and_then(|d| d.latest.get(&Topic::SessionLevels))
            .filter(|f| !f.stale);
        let frame = latest.and_then(|f| match &f.frame.data {
            FrameData::SessionLevels(l) => Some(l),
            _ => None,
        });
        let rig = st.map(|s| s.rig.levels);
        let b = frame
            .map(|f| f.meta.ballistic)
            .or(rig.map(|r| r.ballistic))
            .unwrap_or_default();
        let names: BTreeMap<u16, String> = self.session_input_labels().into_iter().collect();
        let name = |c: u16| {
            names
                .get(&c)
                .cloned()
                .unwrap_or_else(|| ac2_scene::meter::input_name(c, None, None))
        };
        let input_of = |c: u16| {
            let at = frame.and_then(|f| f.meta.channels.iter().position(|x| *x == c));
            let col = |v: &[f32]| at.and_then(|i| v.get(i).copied());
            let device = st
                .and_then(|s| s.session.open.as_ref())
                .map(|o| &o.input_device);
            ProgrammeInput {
                label: name(c),
                scale: self.programme_scale(c),
                ppm: frame.and_then(|f| col(&f.ppm)),
                true_peak_dbtp: frame.and_then(|f| col(&f.true_peak)),
                max: frame.and_then(|f| col(&f.max)),
                overs: frame
                    .and_then(|f| at.and_then(|i| f.overs.get(i).copied()))
                    .unwrap_or(0),
                clipped: frame
                    .and_then(|f| at.and_then(|i| f.clip.get(i)))
                    .is_some_and(|k| *k != ac2_proto::frame::ClipFlags::NONE),
                flying_peak: self.programme.flying.get(&c).and_then(FlyingPeak::value),
                offers_vu: match (st, device) {
                    (Some(s), Some(d)) => {
                        ac2_proto::cal::offers_vu(&s.inputs, &s.line_calibrations, d, c)
                    }
                    _ => false,
                },
            }
        };
        let inputs: Vec<ProgrammeInput> = metered.inputs().into_iter().map(input_of).collect();
        let ballistic = ballistic(b);
        let pair_title = |l: u16, r: u16| {
            st.and_then(|s| s.pairs.iter().find(|p| p.left == l && p.right == r))
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| ac2_scene::meter::pair_name(&name(l), &name(r)))
        };
        let loudness = self.open_session().and_then(|o| {
            let s = st?;
            let pairs: Vec<NamedPair> = s
                .pairs
                .iter()
                .map(|p| NamedPair {
                    left: p.left,
                    right: p.right,
                    name: pair_title(p.left, p.right),
                })
                .collect();
            let captured_at = latest.map(|f| f.frame.stamp.capture_wall_ns);
            let reset_key = self
                .keys
                .first_chord(CommandId::LevelsReset, Scope::Programme)
                .map(|c| c.label());
            let captured = &o.config.input_channels;
            let input = |c: u16| captured.contains(&c).then(|| input_of(c));
            let f = LoudnessFacts {
                rate_hz: o.sample_rate_hz,
                pairs: &pairs,
                captured,
                sources: frame.map_or(&[], |f| f.meta.loudness.as_slice()),
                is_mic: &|c| {
                    matches!(
                        ac2_proto::cal::input_kind(&s.inputs, c),
                        ac2_proto::model::InputKind::Mic { .. }
                    )
                },
                label: &name,
                input: &input,
                own_name: &|c| self.input_own_name(c),
                clock: &|t| {
                    ac2_scene::time::local_clock(t, captured_at.unwrap_or(t), |w| {
                        self.local_zone.offset_s(w)
                    })
                },
                reset_key: reset_key.as_deref(),
            };
            let resolved = sources(&metered.inputs(), &f);
            Some((
                bars_of(&resolved, ballistic, &f),
                plate_readout(&resolved, &f),
            ))
        });
        let (bars, loudness) = loudness.unwrap_or_else(|| (peak_bars(&inputs, ballistic), None));
        let pair = metered.pair().map(|(l, r)| ProgrammePair {
            name: pair_title(l, r),
            correlation: correlation(frame.and_then(|f| {
                f.meta
                    .pair_levels
                    .iter()
                    .find(|p| p.left == l && p.right == r)
            })),
            window_s: rig.map_or(1.0, |r| r.correlation_window.seconds()),
        });
        ProgrammePane {
            inputs,
            bars,
            pair,
            ballistic,
            loudness_target: rig.map_or(-23.0, |r| r.loudness_target.0),
            loudness,
            plate: ac2_scene::plate::PlatePlace::default(),
            plate_hover: None,
        }
    }

    /// The scale input `c` reads on: dB SPL through a mic's calibration, dBu through a line
    /// calibration, dBFS otherwise.
    pub fn programme_scale(&self, c: u16) -> Scale {
        use ac2_proto::cal::Sensitivity;
        let Some(s) = self.daemon() else {
            return Scale::Dbfs;
        };
        match ac2_proto::cal::state_input_use(s, c).sensitivity {
            Sensitivity::Line(e) => Scale::Dbu {
                fs_dbu: ac2_proto::cal::dbu_fs(e.line.v_fs.0).0 as f32,
            },
            Sensitivity::Verified(e) | Sensitivity::OtherMicOrInput(e) => Scale::DbSpl {
                fs_db_spl: e.spl.sensitivity.0 as f32,
            },
            Sensitivity::Uncalibrated => Scale::Dbfs,
        }
    }

    /// What the Inputs-list row of input `c` meters: its pair when `c` is the left of one
    /// the session captures, else `c` alone.
    pub fn row_meters(&self, c: u16) -> Option<Metered> {
        let o = self.open_session()?;
        let captured = &o.config.input_channels;
        let pair = self.daemon().and_then(|s| {
            s.pairs
                .iter()
                .find(|p| p.left == c && captured.contains(&p.right))
        });
        match pair {
            Some(p) => Some(Metered::Pair {
                left: p.left,
                right: p.right,
            }),
            None => captured
                .contains(&c)
                .then(|| Metered::inputs_of(&[c]))
                .flatten(),
        }
    }

    /// The Programme rows of a pane's list: each pair, each input alone, then the first
    /// eight inputs side by side.
    pub fn programme_menu_rows(&self) -> Vec<(crate::state::PaneMenuRow, String)> {
        use crate::state::PaneMenuRow;
        use ac2_scene::programme::{inputs_text, menu_row};
        let Some(o) = self.open_session() else {
            return Vec::new();
        };
        let rows = self.session_inputs();
        let mut v: Vec<(PaneMenuRow, String)> = rows
            .iter()
            .filter_map(|r| {
                Some((
                    PaneMenuRow::Meter(self.row_meters(r.channel)?),
                    menu_row(&r.label),
                ))
            })
            .collect();
        // A pair's sides alone as well: a row lists the pair, not its right input.
        for r in rows.iter().filter(|r| r.right.is_some()) {
            for c in [Some(r.channel), r.right.as_ref().map(|x| x.0)]
                .into_iter()
                .flatten()
            {
                if let Some(m) = Metered::inputs_of(&[c]) {
                    let name = self
                        .session_input_labels()
                        .into_iter()
                        .find(|(x, _)| *x == c)
                        .map_or_else(|| inputs_text(&[c]), |(_, n)| n);
                    v.push((PaneMenuRow::Meter(m), menu_row(&name)));
                }
            }
        }
        let all: Vec<u16> = o
            .config
            .input_channels
            .iter()
            .copied()
            .take(MAX_METERED)
            .collect();
        if all.len() > 1
            && let Some(m) = Metered::inputs_of(&all)
        {
            v.push((PaneMenuRow::Meter(m), menu_row(&inputs_text(&all))));
        }
        v
    }

    /// A Programme pane of `m` split off the focused pane (which keeps what it shows), with
    /// the focus.
    pub(in crate::state) fn open_programme(&mut self, m: Metered) {
        let Some(new) = self.split_focused_pane() else {
            let close = self.keys.reach(crate::keys::CommandId::ClosePane);
            self.warn(format!(
                "no room to split the pane for the meters: {close} closes one to make room"
            ));
            return;
        };
        self.set_pane_kind(new, PaneKind::Programme);
        if let Some(v) = self.layout.view_mut(new) {
            v.meas = None;
            v.meters = Some(m);
        }
        self.focus_pane(new);
    }

    /// The inputs R resets: the picked Inputs row's, else the focused Programme pane's.
    pub(in crate::state) fn reset_target(&self) -> Option<Vec<u16>> {
        match self.input_pick {
            Some(c) => self.row_meters(c).map(|m| m.inputs()),
            None => self
                .layout
                .view(self.layout.focus)
                .filter(|v| v.kind == PaneKind::Programme)
                .and_then(|v| v.meters)
                .map(|m| m.inputs()),
        }
    }

    /// `levels.reset` of `channels` (empty: every input), with what the toast says.
    pub(in crate::state) fn levels_reset(&mut self, channels: Vec<u16>, out: &mut Vec<Request>) {
        if self.open_session().is_none() {
            self.fault("no session is open: nothing to reset");
            return;
        }
        let what = if channels.is_empty() {
            "max, overs and loudness reset on every input".to_owned()
        } else {
            let n: Vec<String> = channels
                .iter()
                .map(|c| (u32::from(*c) + 1).to_string())
                .collect();
            format!("max, overs and loudness reset on input {}", n.join(", "))
        };
        out.push(Request::Call {
            cmd: ac2_proto::Command::LevelsReset { channels },
            what,
        });
    }
}
