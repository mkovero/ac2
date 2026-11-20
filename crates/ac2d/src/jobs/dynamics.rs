//! Dynamics job: feeds the reference and every measured input of a `dynamics.run` to the
//! streaming analysis as the audio arrives, publishes the rests it finds recovered to the
//! programme source (one atomic store), reports progress, and hands the result to the
//! control thread once the last rest is in. Only the step in progress is kept, inside the
//! analysis; nothing here holds the whole run.
//!
//! Audio lost while recording (a gap in the sample counter, an xrun or overflow flag) ends
//! the run as a dropout: an edge or a release time measured across a gap would be wrong,
//! never missing.

use std::sync::mpsc::Sender;

use ac2_core::dynamics::{Analyser, DynamicsError, Phase, Release};
use ac2_proto::model::DynamicsFailure;
use ac2_proto::units::DynamicsId;

use super::{Analysis, Emitter, Flush, JobCmd, StampArgs};
use crate::control::ControlMsg;
use crate::fanout::Block;

/// Why a run ended without a result, with the detail for the operator.
pub(crate) type Failure = (DynamicsFailure, String);

pub(crate) struct Recorder {
    id: DynamicsId,
    ref_idx: usize,
    in_idx: Vec<usize>,
    analyser: Option<Analyser>,
    release: Release,
    reference: Vec<f32>,
    inputs: Vec<Vec<f32>>,
    end: Option<u64>,
    phase: Phase,
    to: Sender<ControlMsg>,
}

impl Recorder {
    pub(crate) fn new(
        id: DynamicsId,
        ref_idx: usize,
        in_idx: Vec<usize>,
        analyser: Analyser,
        release: Release,
        to: Sender<ControlMsg>,
    ) -> Self {
        let inputs = in_idx.iter().map(|_| Vec::new()).collect();
        Self {
            id,
            ref_idx,
            in_idx,
            analyser: Some(analyser),
            release,
            reference: Vec::new(),
            inputs,
            end: None,
            phase: Phase::Waiting,
            to,
        }
    }

    fn finish(&mut self, result: Result<ac2_core::dynamics::DynamicsResult, Failure>) {
        self.analyser = None;
        let _ = self.to.send(ControlMsg::DynamicsDone {
            id: self.id,
            result: Box::new(result),
        });
    }
}

/// The run's failure for an analysis error.
pub(crate) fn failure(e: &DynamicsError) -> Failure {
    let reason = match e {
        DynamicsError::NoReference => DynamicsFailure::NoReference,
        DynamicsError::PilotNotBelowThreshold { .. } => DynamicsFailure::PilotNotBelowThreshold,
        DynamicsError::Mismatch(_) => DynamicsFailure::Mismatch,
    };
    (reason, e.to_string())
}

impl Analysis for Recorder {
    fn result_generation(&self) -> Option<u64> {
        // Not an operand of math traces.
        None
    }

    fn push(&mut self, b: &Block) {
        if self.analyser.is_none() {
            return;
        }
        if let Some(e) = self.end
            && (b.start_sample != e || b.flags.breaks_continuity())
        {
            self.finish(Err((
                DynamicsFailure::Dropout,
                format!(
                    "audio was lost at sample {} while recording; run it again",
                    b.start_sample
                ),
            )));
            return;
        }
        self.end = Some(b.end_sample());
        let n = usize::from(b.channels).max(1);
        self.reference.clear();
        for v in &mut self.inputs {
            v.clear();
        }
        for frame in b.data.chunks(n) {
            self.reference.push(frame[self.ref_idx]);
            for (v, &i) in self.inputs.iter_mut().zip(&self.in_idx) {
                v.push(frame[i]);
            }
        }
        let Some(a) = self.analyser.as_mut() else {
            return;
        };
        let inputs: Vec<&[f32]> = self.inputs.iter().map(Vec::as_slice).collect();
        if let Err(e) = a.push(&self.reference, &inputs) {
            self.finish(Err(failure(&e)));
            return;
        }
        self.release.publish(a.released());
        let phase = a.phase();
        if phase != self.phase {
            self.phase = phase;
            let _ = self
                .to
                .send(ControlMsg::DynamicsProgress { id: self.id, phase });
        }
        if phase == Phase::Done
            && let Some(a) = self.analyser.take()
        {
            let r = a.finish().map_err(|e| failure(&e));
            self.finish(r);
        }
    }

    fn command(&mut self, _c: JobCmd) {}

    fn emit(&mut self, _e: &Emitter) -> Flush {
        Flush::Done
    }

    fn capture(&mut self) -> Option<(StampArgs, ac2_proto::frame::FrameData)> {
        None
    }
}
