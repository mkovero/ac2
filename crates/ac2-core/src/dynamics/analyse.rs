//! The streaming analysis of a dynamics run.

use super::{DynamicsError, DynamicsResult, Phase, Programme};

/// Analyses a dynamics run as its audio arrives: feed the reference and every measured input
/// block by block ([`Analyser::push`]), publish [`Analyser::released`] to the programme's
/// [`super::Release`], and take the result with [`Analyser::finish`] once the phase is
/// [`Phase::Done`].
#[derive(Debug)]
pub struct Analyser {
    _programme: Programme,
}

impl Analyser {
    /// An analyser of `programme` with `inputs` measured inputs (1 or more).
    pub fn new(programme: &Programme, inputs: usize) -> Self {
        let _ = inputs;
        Self {
            _programme: programme.clone(),
        }
    }

    /// Feeds the next block: the reference and each measured input, equal lengths,
    /// contiguous with the previous block.
    pub fn push(&mut self, reference: &[f32], inputs: &[&[f32]]) -> Result<(), DynamicsError> {
        let _ = (reference, inputs);
        todo!()
    }

    /// Rests found recovered so far (1-based count, monotonic): what the source may end.
    pub fn released(&self) -> u32 {
        todo!()
    }

    /// Where the run is.
    pub fn phase(&self) -> Phase {
        todo!()
    }

    /// The result, once [`Phase::Done`].
    pub fn finish(self) -> Result<DynamicsResult, DynamicsError> {
        todo!()
    }
}
