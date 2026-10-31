//! A room simulation's texts (`docs/design/spatial-walk.md` §7): what it is (*simulated*),
//! what it assumes (*absorption assumed*, *omnidirectional source*), where it holds
//! (*simulation not valid below 180 Hz*), its progress, and its room parameters as a table.
//! A simulation is never a trace: these texts are its only face besides the plot overlays.

use ac2_proto::model::{
    SimAbsorption, SimCancel, SimFitKept, SimKind, SimMap, SimStage, SimState, SimStatus,
};

use super::{BandSet, RoomTable, walk};
use crate::format;

/// What every simulated thing is labelled.
pub const SIMULATED: &str = "simulated";

/// *absorption assumed*, or *absorption fitted to 4 positions*, naming the bands the walk
/// gave no T20 in (they keep the table's absorption).
pub fn absorption_label(s: &SimState) -> String {
    match &s.absorption {
        SimAbsorption::Assumed => "absorption assumed".to_owned(),
        SimAbsorption::Fitted {
            positions, bands, ..
        } => {
            let mut t = format!(
                "absorption fitted to {positions} position{}",
                if *positions == 1 { "" } else { "s" }
            );
            let assumed: Vec<String> = bands
                .iter()
                .zip(&s.bands)
                .filter(|(b, _)| b.scale.is_none())
                .map(|(_, f)| format::freq_tick(f.0))
                .collect();
            if !assumed.is_empty() {
                t.push_str(&format!(" · assumed at {} Hz", assumed.join(", ")));
            }
            t
        }
    }
}

/// *simulation not valid below 180 Hz* (the Schroeder frequency, an estimate: two
/// significant digits).
pub fn validity_label(s: &SimState) -> Option<String> {
    s.valid_above
        .map(|f| format!("simulation not valid below {}", format::freq_rough(f.0)))
}

/// *omnidirectional source* (or *sources*): the speakers are points radiating alike in
/// every direction, which a PA box is not.
pub fn source_label(s: &SimState) -> &'static str {
    if s.sources.len() > 1 {
        "omnidirectional sources"
    } else {
        "omnidirectional source"
    }
}

/// The labels a simulated result always carries, in order.
pub fn labels(s: &SimState) -> Vec<String> {
    let mut v = vec![SIMULATED.to_owned(), absorption_label(s)];
    v.extend(validity_label(s));
    v.push(source_label(s).to_owned());
    if s.budget_exhausted {
        v.push(format!(
            "time budget ran out: {} rays per band",
            s.rays_per_band
        ));
    }
    v
}

/// Where the simulation is: *simulating · ray tracing 40 %*, *fitting · pass 2 · …*,
/// *simulated*, *cancelled: the plan changed*, *failed: …*.
pub fn status_text(s: &SimState) -> String {
    match &s.status {
        SimStatus::Running {
            stage,
            done,
            total,
            pass,
            ..
        } => {
            let stage = match stage {
                SimStage::ImageSources => "image sources",
                SimStage::RayTracing => "ray tracing",
                SimStage::Analysis => "room parameters",
            };
            let pct = if *total > 0 {
                format!(" {} %", (100 * done / total).min(100))
            } else {
                String::new()
            };
            if *pass > 0 {
                format!("fitting · pass {pass} · {stage}{pct}")
            } else {
                format!("simulating · {stage}{pct}")
            }
        }
        SimStatus::Done => SIMULATED.to_owned(),
        SimStatus::Cancelled { why } => format!(
            "cancelled: {}",
            match why {
                SimCancel::Operator => "stopped",
                SimCancel::PlanChanged => "the plan changed",
                SimCancel::WalkDeleted => "its walk was deleted",
                SimCancel::SessionClosed => "the session closed",
            }
        ),
        SimStatus::Failed { msg } => format!("failed: {msg}"),
    }
}

/// The room parameters of source `source` over the receivers: per band the mean, with the
/// least and greatest under it. Bands reaching below the validity limit show their values
/// in parentheses: there the room is modal and rays are the wrong model. The legend
/// carries [`labels`].
pub fn room_table(s: &SimState, source: usize) -> Option<RoomTable> {
    let src = s.sources.get(source)?;
    let room = src.room.as_ref()?;
    let mut t = walk::means_table(room, BandSet::Octave, false);
    bracket_modal(&mut t, s, room.octave.iter().map(|b| b.centre));
    t.caption = format!(
        "Room ({SIMULATED}) · {} · mean of {} receiver{} · octave bands",
        src.name,
        room.positions,
        if room.positions == 1 { "" } else { "s" }
    );
    t.legend.extend(labels(s));
    Some(t)
}

/// The room parameters `s` simulated at receiver `label` from its first source (the one a
/// trace captured there is set beside), bands below the validity limit bracketed.
pub fn receiver_table(s: &SimState, label: &str) -> Option<RoomTable> {
    let src = s.sources.first()?;
    let r = src.receivers.iter().find(|r| r.label == label)?;
    let mut t = super::room_table(&r.room, BandSet::Octave);
    bracket_modal(&mut t, s, r.room.octave.iter().map(|b| b.centre));
    t.caption = format!("Room ({SIMULATED}) · {label} · {} · octave bands", src.name);
    t.legend.retain(|l| !l.starts_with("ISO 3382-1"));
    t.legend.extend(labels(s));
    Some(t)
}

/// Brackets the cells of bands (centres `centres`, then broadband) reaching below `s`'s
/// validity limit and says why in the legend.
fn bracket_modal(
    t: &mut RoomTable,
    s: &SimState,
    centres: impl Iterator<Item = Option<ac2_proto::units::Hz>>,
) {
    let limit = s.valid_above.map_or(0.0, |f| f.0);
    let below: Vec<bool> = centres
        .map(|c| c.is_some_and(|c| c.0 / std::f64::consts::SQRT_2 < limit))
        .chain(std::iter::once(false))
        .collect();
    for row in &mut t.rows {
        for (cell, &b) in row.cells.iter_mut().zip(&below) {
            if b && cell.refused.is_none() {
                cell.text = format!("({})", cell.text);
            }
        }
    }
    if below.iter().any(|&b| b) {
        t.legend.push(format!(
            "(…): below {} the room is modal; the simulation is not valid there",
            format::freq_rough(limit)
        ));
    }
}

/// A coverage map's legend: what it shows, then where it holds.
/// *simulated direct + early energy (50 ms) · 1k Hz · omnidirectional source · absorption
/// assumed*, *0 to −40 dB re the loudest cell · speakers add as power, no interference ·
/// not valid below 180 Hz*.
pub fn map_legend(s: &SimState) -> Vec<String> {
    let Some(m) = &s.map else {
        return Vec::new();
    };
    let band = m.band.map_or_else(
        || "broadband".to_owned(),
        |b| format!("{} Hz", format::freq_tick(b.0)),
    );
    let mut first = format!(
        "{SIMULATED} direct + early energy ({:.0} ms) · {band} · {} · {}",
        m.window.0 * 1000.0,
        source_label(s),
        absorption_label(s)
    );
    if s.sources.len() > 1 {
        first.push_str(&format!(" · {} speakers summed", s.sources.len()));
    }
    let mut second = format!(
        "0 to {} dB re the loudest cell",
        format::fixed(f64::from(SimMap::FLOOR_DB), 0)
    );
    if s.sources.len() > 1 {
        second.push_str(" · speakers add as power, no interference");
    }
    if let Some(v) = validity_label(s) {
        second.push_str(&format!(" · {v}"));
    }
    vec![first, second]
}

/// *812 cells of 1.0 m · median −6.1 dB, quietest −18.2 dB re the loudest*.
pub fn map_summary(m: &SimMap) -> String {
    let mut db: Vec<f32> = m.cells.iter().map(|c| c.db).collect();
    db.sort_by(f32::total_cmp);
    let median = db.get(db.len() / 2).copied().unwrap_or(0.0);
    let least = db.first().copied().unwrap_or(0.0);
    format!(
        "{} cells of {:.1} m · median {} dB, quietest {} dB re the loudest",
        m.cells.len(),
        m.spacing,
        format::signed(f64::from(median), 1),
        format::signed(f64::from(least), 1)
    )
}

/// The walk pane's simulation line for the newest of `sims` (a run or fit, a map) on the
/// walk's plan, and the fit the daemon keeps: *Simulation: fitting · pass 2 · ray tracing
/// 40 %*, *Coverage map: simulated · absorption assumed*, *Simulation: absorption fitted to
/// 4 positions, kept for the next run*. `None` when neither concerns `plan`.
pub fn status_line(
    sims: &[Option<&SimState>],
    fit: Option<&SimFitKept>,
    plan: &str,
) -> Option<String> {
    let fit = fit.filter(|f| f.plan == plan);
    let fit_words = |f: &SimFitKept| match &f.absorption {
        SimAbsorption::Fitted { positions, .. } => format!(
            "absorption fitted to {positions} position{}",
            if *positions == 1 { "" } else { "s" }
        ),
        SimAbsorption::Assumed => "absorption assumed".to_owned(),
    };
    let newest = sims
        .iter()
        .flatten()
        .filter(|s| s.plan == plan)
        .max_by_key(|s| s.id.0);
    let Some(s) = newest else {
        return fit.map(|f| format!("Simulation: {}, kept for the next run", fit_words(f)));
    };
    let what = if s.kind == SimKind::Map {
        "Coverage map"
    } else {
        "Simulation"
    };
    let mut t = format!("{what}: {}", status_text(s));
    if s.status == SimStatus::Done {
        t.push_str(&format!(" · {}", absorption_label(s)));
        if let (SimAbsorption::Assumed, Some(f)) = (&s.absorption, fit) {
            t.push_str(&format!(" · {}, kept for the next run", fit_words(f)));
        }
    }
    Some(t)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ac2_proto::model::{SimFitBand, SimStatus};
    use ac2_proto::samples::{sim_running, sim_state};
    use ac2_proto::units::{Hz, Seconds};

    #[test]
    fn a_fit_names_its_positions_and_the_bands_it_left_assumed() {
        let s = sim_state();
        assert_eq!(
            absorption_label(&s),
            "absorption fitted to 4 positions · assumed at 1k Hz"
        );
        assert_eq!(
            labels(&s),
            [
                "simulated",
                "absorption fitted to 4 positions · assumed at 1k Hz",
                "simulation not valid below 75 Hz",
                "omnidirectional source",
            ]
        );
    }

    #[test]
    fn an_unfitted_run_says_absorption_assumed() {
        let mut s = sim_state();
        s.absorption = SimAbsorption::Assumed;
        s.valid_above = Some(Hz(183.4));
        assert_eq!(absorption_label(&s), "absorption assumed");
        assert_eq!(
            validity_label(&s).unwrap(),
            "simulation not valid below 180 Hz"
        );
        s.budget_exhausted = true;
        assert_eq!(
            labels(&s).last().unwrap(),
            "time budget ran out: 20480 rays per band"
        );
    }

    #[test]
    fn progress_reads_stage_and_percent() {
        assert_eq!(status_text(&sim_running()), "simulating · ray tracing 20 %");
        let mut s = sim_running();
        if let SimStatus::Running { pass, .. } = &mut s.status {
            *pass = 2;
        }
        assert_eq!(status_text(&s), "fitting · pass 2 · ray tracing 20 %");
        s.status = SimStatus::Cancelled {
            why: SimCancel::PlanChanged,
        };
        assert_eq!(status_text(&s), "cancelled: the plan changed");
        assert_eq!(status_text(&sim_state()), "simulated");
    }

    #[test]
    fn the_table_is_captioned_simulated_and_brackets_modal_bands() {
        let mut s = sim_state();
        s.absorption = SimAbsorption::Fitted {
            walk: ac2_proto::units::WalkId(2),
            positions: 4,
            bands: vec![
                SimFitBand {
                    scale: Some(1.0),
                    measured: Some(Seconds(1.0)),
                    simulated: Some(Seconds(1.0)),
                    converged: true,
                };
                2
            ],
        };
        s.valid_above = Some(Hz(200.0));
        s.sources[0].room = Some(walk::tests::walk_room());
        let t = room_table(&s, 0).unwrap();
        assert_eq!(
            t.caption,
            "Room (simulated) · Main L · mean of 4 receivers · octave bands"
        );
        assert_eq!(t.bands, ["125", "All"]);
        // 125 Hz lies below 200 Hz: bracketed; broadband is not.
        assert_eq!(t.rows[0].cells[0].text, "(1.20)");
        assert_eq!(t.rows[0].cells[1].text, "0.90");
        assert!(t.legend.iter().any(|l| l.starts_with("(…): below 200 Hz")));
        assert!(t.legend.iter().all(|l| !l.starts_with("ISO 3382-1")));
        assert_eq!(
            &t.legend[t.legend.len() - 4..],
            [
                "simulated",
                "absorption fitted to 4 positions",
                "simulation not valid below 200 Hz",
                "omnidirectional source",
            ]
        );
        assert!(room_table(&s, 1).is_none());
    }

    #[test]
    fn a_coverage_map_says_what_it_shows_and_where_it_holds() {
        use ac2_proto::samples::sim_map_state;
        let mut s = sim_map_state();
        s.absorption = SimAbsorption::Assumed;
        s.valid_above = Some(Hz(183.0));
        assert_eq!(
            map_legend(&s),
            [
                "simulated direct + early energy (50 ms) · 1k Hz · omnidirectional source · \
                 absorption assumed",
                "0 to -40 dB re the loudest cell · simulation not valid below 180 Hz",
            ]
            .map(|l| l.replace('-', &format::MINUS.to_string()))
        );
        s.sources.push(s.sources[0].clone());
        s.map.as_mut().unwrap().band = None;
        let l = map_legend(&s);
        assert!(
            l[0].contains("broadband · omnidirectional sources"),
            "{l:?}"
        );
        assert!(l[0].ends_with("2 speakers summed"), "{l:?}");
        assert!(
            l[1].contains("speakers add as power, no interference"),
            "{l:?}"
        );
        assert_eq!(
            map_summary(s.map.as_ref().unwrap()),
            format!(
                "3 cells of 1.0 m · median {m}1.5 dB, quietest {m}6.2 dB re the loudest",
                m = format::MINUS
            )
        );
        assert!(map_legend(&sim_state()).is_empty());
    }

    #[test]
    fn the_walk_status_line_follows_the_newest_on_its_plan() {
        use ac2_proto::samples::{sim_fit_kept, sim_map_state};
        let run = sim_running();
        let map = sim_map_state();
        let fit = sim_fit_kept();
        assert_eq!(
            status_line(&[Some(&run)], None, "Hall").unwrap(),
            "Simulation: simulating · ray tracing 20 %"
        );
        // The map (id 5) is newer than the run (id 4); the kept fit is named after an
        // assumed result.
        let mut assumed = map.clone();
        assumed.absorption = SimAbsorption::Assumed;
        assert_eq!(
            status_line(&[Some(&run), Some(&assumed)], Some(&fit), "Hall").unwrap(),
            "Coverage map: simulated · absorption assumed · absorption fitted to 4 positions, \
             kept for the next run"
        );
        assert_eq!(
            status_line(&[None, None], Some(&fit), "Hall").unwrap(),
            "Simulation: absorption fitted to 4 positions, kept for the next run"
        );
        assert_eq!(status_line(&[Some(&run)], Some(&fit), "Club"), None);
    }

    #[test]
    fn a_receivers_table_is_its_own_and_labelled_simulated() {
        let s = sim_state();
        let t = receiver_table(&s, "P1").unwrap();
        assert_eq!(t.caption, "Room (simulated) · P1 · Main L · octave bands");
        assert!(t.legend.iter().any(|l| l == "simulated"));
        assert!(receiver_table(&s, "P9").is_none());
    }
}
