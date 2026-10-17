//! Panes stacked in one column that share the frequency axis: each plot's right margin is
//! its own room (a colour bar, an overlay axis) or the widest of the column's, so one
//! frequency sits at one screen x in all of them and a click in one moves the cursor to the
//! same place in the others.

use crate::canvas::MARGINS;
use crate::view::ViewState;

/// A picture with the log-frequency x axis, by the room it keeps right of its plot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreqPlot {
    /// The spectrum or RTA, with the spectrograph or without (`view.spectrum.mode`).
    Spectrum,
    /// The transfer function's magnitude, phase and coherence plots.
    Transfer,
    /// The sweep's fundamental and distortion.
    Distortion,
}

/// What `plot` keeps right of its plot in `view` when it stands alone.
pub fn natural_right(plot: FreqPlot, view: &ViewState) -> f32 {
    match plot {
        FreqPlot::Spectrum if view.spectrum.mode.spectrograph() => {
            crate::spectrograph::RIGHT_MARGIN
        }
        FreqPlot::Transfer => crate::tf::right_margin(view),
        FreqPlot::Spectrum | FreqPlot::Distortion => MARGINS.right,
    }
}

/// The right margin a picture with `natural` room of its own draws with in `view`: at
/// least what its column reserves ([`ViewState::freq_right`]).
pub(crate) fn right(natural: f32, view: &ViewState) -> f32 {
    natural.max(view.freq_right)
}
