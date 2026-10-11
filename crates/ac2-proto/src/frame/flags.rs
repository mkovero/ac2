//! Bitmask newtypes carried in u32 array parts and in the header.

use serde::{Deserialize, Serialize};

use crate::model::LeqJudgement;

macro_rules! bitmask {
    ($(#[$doc:meta])* $name:ident { $($(#[$fdoc:meta])* $flag:ident = $bit:expr;)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// No bit set.
            pub const NONE: Self = Self(0);
            $($(#[$fdoc])* pub const $flag: Self = Self($bit);)*
            /// Every defined bit.
            pub const ALL: Self = Self(0 $(| $bit)*);
            /// `(name, bit)` of every flag, for documentation and other languages.
            pub const NAMED: &'static [(&'static str, u32)] = &[$((stringify!($flag), $bit)),*];

            /// True when all bits of `o` are set.
            pub fn contains(self, o: Self) -> bool {
                self.0 & o.0 == o.0
            }
            /// Union.
            pub fn with(self, o: Self) -> Self {
                Self(self.0 | o.0)
            }
            /// True when only defined bits are set.
            pub fn is_known(self) -> bool {
                self.0 & !Self::ALL.0 == 0
            }
        }
    };
}

bitmask!(
    /// Why a column has no value (0 = valid). Bits combine.
    ValidityMask {
        /// No bin of the serving stage falls in the column.
        THINNED = 1 << 0;
        /// Above the highest served frequency.
        OUT_OF_BAND = 1 << 1;
        /// No averaged block yet.
        SETTLING = 1 << 2;
        /// No reference energy in the column.
        NO_REFERENCE = 1 << 3;
        /// No measurement energy in the column.
        NO_MEASUREMENT = 1 << 4;
        /// Withheld by input protection (clip, missing reference).
        PROTECTED = 1 << 5;
        /// Below the noise / coherence floor.
        BELOW_FLOOR = 1 << 6;
        /// Band narrower than two bins.
        INSUFFICIENT_RESOLUTION = 1 << 7;
        /// Band above Nyquist.
        ABOVE_NYQUIST = 1 << 8;
        /// A math channel without the usable operands it needs: no value.
        FEW_OPERANDS = 1 << 9;
    }
);

bitmask!(
    /// Input protection state of the measurement producing the frame.
    ProtectionFlags {
        /// Reference below its floor: blocks rejected.
        NO_REFERENCE = 1 << 0;
        /// Measurement below its floor.
        NO_SIGNAL = 1 << 1;
        /// Clipping (held).
        CLIP = 1 << 2;
        /// Reference weak relative to the measurement.
        WEAK_REFERENCE = 1 << 3;
        /// Averages restarted after a stream discontinuity.
        DISCONTINUITY = 1 << 4;
        /// Reference and measurement look mis-patched: identical or near-perfectly
        /// correlated at zero lag (the same signal on both inputs; a real acoustic path
        /// always has propagation delay), or the reference silent while the measurement
        /// carries signal (reference and measurement swapped).
        CHECK_ROUTING = 1 << 5;
    }
);

bitmask!(
    /// State of a rolling Leq window. Its judgement ([`LeqFlags::judgement`]) is "no
    /// limit" without `LIMIT`, "not calibrated" with `LIMIT` but without `JUDGED`, else
    /// over, near or ok.
    LeqFlags {
        /// The window has a limit.
        LIMIT = 1 << 0;
        /// The limit is judged (the meter reads dB SPL).
        JUDGED = 1 << 1;
        /// Within the warn margin below the limit, or at it; or, filling, on course
        /// (`ON_COURSE`).
        NEAR = 1 << 2;
        /// Above the limit; while filling, the energy so far has spent the whole window's
        /// budget (it ends over even if the rest is silent).
        OVER = 1 << 3;
        /// No steady level over the horizon brings the window to its limit.
        CANNOT_RECOVER = 1 << 4;
        /// Part of the window was not measured (capture gaps, the meter stopped).
        INCOMPLETE = 1 << 5;
        /// Filling, with the Leq so far above the limit: at the pace so far the full window
        /// ends over it (with `NEAR`).
        ON_COURSE = 1 << 6;
    }
);

impl LeqFlags {
    /// The judgement the flags carry.
    pub fn judgement(self) -> LeqJudgement {
        if !self.contains(Self::LIMIT) {
            LeqJudgement::NoLimit
        } else if !self.contains(Self::JUDGED) {
            LeqJudgement::NotCalibrated
        } else if self.contains(Self::OVER) {
            LeqJudgement::Over
        } else if self.contains(Self::NEAR) {
            LeqJudgement::Near
        } else {
            LeqJudgement::Ok
        }
    }
}

bitmask!(
    /// Per-channel clip state.
    ClipFlags {
        /// Clipped in this frame's interval.
        CLIP = 1 << 0;
        /// Clip indicator held.
        HELD = 1 << 1;
    }
);

/// Bitmask newtypes stored in u32 array parts.
pub(super) trait MaskBits: Sized {
    fn to_u32(&self) -> u32;
    fn from_u32(x: u32) -> Self;
    fn known(&self) -> bool;
}

macro_rules! mask_bits {
    ($t:ident) => {
        impl MaskBits for $t {
            fn to_u32(&self) -> u32 {
                self.0
            }
            fn from_u32(x: u32) -> Self {
                Self(x)
            }
            fn known(&self) -> bool {
                self.is_known()
            }
        }
    };
}

mask_bits!(ValidityMask);
mask_bits!(ClipFlags);
mask_bits!(LeqFlags);
