//! A list row dropped onto another row: the math channel that combines them, or why they do
//! not combine. Nothing is made by the drop itself: it opens the math dialog with A (the row
//! dropped on), the operator and B (the row dragged) filled in.
//!
//! The operator is the reading a curve of that kind asks for. An EQ set is a filter and a
//! loudness shift a change in how a level is heard: both act on the curve (`×`, "what this
//! does to that curve"). A target is a goal: the curve is read against it (`÷`, "how far
//! the curve is from the target"). Two measured transfer functions are compared (`÷`, the
//! dialog's first operator), two spectra differenced in level (`−`, likewise); a stored
//! transfer function over a spectrum filters it (`×`, "through B").

use ac2_proto::model::{MathDomain, MathOp};

use crate::math::op_symbol;

/// What a list row is, as far as combining goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropCurve {
    /// A measurement's live curve (or its header).
    Live,
    /// A stored measured trace: a capture, sweep run, import, average.
    Stored,
    /// A math channel: its result is no operand until captured.
    Math,
    /// An EQ set (its curve's row).
    Eq,
    /// A generated loudness shift.
    Loudness,
    /// A target: flat, a house tilt, or one loaded from a file.
    Target,
}

/// One side of the drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropSide<'a> {
    pub name: &'a str,
    /// The kind of curve it is, `None` with no curve that combines (an SPL meter, a sweep
    /// measurement's header: its runs are the curves).
    pub domain: Option<MathDomain>,
    pub curve: DropCurve,
}

/// `transfer function`, `spectrum`, `RTA`: as the math dialog names the kinds.
pub fn domain_word(d: MathDomain) -> &'static str {
    match d {
        MathDomain::Transfer => "transfer function",
        MathDomain::Spectrum => "spectrum",
        MathDomain::Rta => "RTA",
    }
}

impl DropCurve {
    /// An EQ set or a generated or loaded curve: something a measured curve is read
    /// through or against, never the other way round.
    pub fn is_reference(self) -> bool {
        matches!(
            self,
            DropCurve::Eq | DropCurve::Loudness | DropCurve::Target
        )
    }
}

/// Whether the drop is read the other way round: a measured curve dropped on an EQ set, a
/// loudness shift or a target is A all the same (a curve goes through a set, is heard at
/// another level, is read against a target), so the drop works in either direction.
pub fn swaps(onto: &DropSide<'_>, dragged: &DropSide<'_>) -> bool {
    onto.curve.is_reference() && matches!(dragged.curve, DropCurve::Live | DropCurve::Stored)
}

/// The operator a drop of `b` onto `a` opens the dialog with (`a` is A), or why the two do
/// not combine.
pub fn plan(a: &DropSide<'_>, b: &DropSide<'_>) -> Result<MathOp, String> {
    if a.name == b.name && a.curve == b.curve && a.domain == b.domain {
        return Err(format!("drop {} on another curve to combine them", b.name));
    }
    for s in [b, a] {
        if s.curve == DropCurve::Math {
            return Err(format!(
                "{} is a math channel: capture it to combine it with another curve",
                s.name
            ));
        }
    }
    let (Some(da), Some(db)) = (a.domain, b.domain) else {
        let none = if b.domain.is_none() { b } else { a };
        return Err(format!(
            "{} has no curve to combine: drop a live curve, a stored trace, an EQ set or a \
             target",
            none.name
        ));
    };
    if a.curve == DropCurve::Eq {
        return Err(format!(
            "{} and {} are both EQ sets: drop one on a curve",
            a.name, b.name
        ));
    }
    match (da, db) {
        (MathDomain::Transfer, MathDomain::Transfer) => Ok(match b.curve {
            DropCurve::Eq | DropCurve::Loudness => MathOp::Multiply,
            DropCurve::Target | DropCurve::Live | DropCurve::Stored | DropCurve::Math => {
                MathOp::Divide
            }
        }),
        (MathDomain::Transfer, other) => Err(format!(
            "{} is a {}: it does not combine with {}, a transfer function",
            b.name,
            domain_word(other),
            a.name
        )),
        (level, MathDomain::Transfer) => match b.curve {
            DropCurve::Eq => Err(format!(
                "{} is an EQ set: it filters transfer functions, and {} is a{} {}",
                b.name,
                a.name,
                if level == MathDomain::Rta { "n" } else { "" },
                domain_word(level)
            )),
            DropCurve::Live => Err(format!(
                "a {} is filtered by a stored transfer function: capture {} first",
                domain_word(level),
                b.name
            )),
            _ => Ok(MathOp::Multiply),
        },
        (x, y) if x == y => Ok(MathOp::Subtract),
        (x, y) => Err(format!(
            "{} is a{} {} and {} a{} {}: they do not combine",
            a.name,
            if x == MathDomain::Rta { "n" } else { "" },
            domain_word(x),
            b.name,
            if y == MathDomain::Rta { "n" } else { "" },
            domain_word(y)
        )),
    }
}

/// The label by the pointer over a row the drop combines with: `math channel: TF 1 S1 ×
/// loudness 85 → 100 phon`, `math channel: Main L after EQ 1`.
pub fn label(a: &str, op: MathOp, b: &str, b_is_eq: bool) -> String {
    if b_is_eq && op == MathOp::Multiply {
        format!("math channel: {a} after {b}")
    } else {
        format!("math channel: {a} {} {b}", op_symbol(op))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(name: &str, domain: Option<MathDomain>, curve: DropCurve) -> DropSide<'_> {
        DropSide {
            name,
            domain,
            curve,
        }
    }

    const TF: Option<MathDomain> = Some(MathDomain::Transfer);
    const SPEC: Option<MathDomain> = Some(MathDomain::Spectrum);

    #[test]
    fn a_curve_kind_picks_the_reading_it_asks_for() {
        let s1 = side("TF 1 S1", TF, DropCurve::Stored);
        let cases = [
            (side("EQ 1", TF, DropCurve::Eq), MathOp::Multiply),
            (
                side("loudness 85 → 100 phon", TF, DropCurve::Loudness),
                MathOp::Multiply,
            ),
            (side("flat", TF, DropCurve::Target), MathOp::Divide),
            (side("house", TF, DropCurve::Target), MathOp::Divide),
            (side("TF 2 S1", TF, DropCurve::Stored), MathOp::Divide),
            (side("TF 2", TF, DropCurve::Live), MathOp::Divide),
        ];
        for (b, op) in cases {
            assert_eq!(plan(&s1, &b), Ok(op), "{}", b.name);
        }
        let spec = side("Room", SPEC, DropCurve::Live);
        assert_eq!(
            plan(&spec, &side("Room S1", SPEC, DropCurve::Stored)),
            Ok(MathOp::Subtract)
        );
        assert_eq!(
            plan(&spec, &side("mic IR", TF, DropCurve::Stored)),
            Ok(MathOp::Multiply)
        );
    }

    #[test]
    fn what_does_not_combine_says_why_in_the_dialogs_words() {
        let tf = side("TF 1", TF, DropCurve::Live);
        let spec = side("Room", SPEC, DropCurve::Live);
        let eq = side("EQ 1", TF, DropCurve::Eq);
        assert_eq!(
            plan(&tf, &spec).err().as_deref(),
            Some("Room is a spectrum: it does not combine with TF 1, a transfer function")
        );
        assert_eq!(
            plan(&spec, &eq).err().as_deref(),
            Some("EQ 1 is an EQ set: it filters transfer functions, and Room is a spectrum")
        );
        assert_eq!(
            plan(&spec, &tf).err().as_deref(),
            Some("a spectrum is filtered by a stored transfer function: capture TF 1 first")
        );
        assert_eq!(
            plan(&tf, &side("TF 1 ÷ TF 2", TF, DropCurve::Math))
                .err()
                .as_deref(),
            Some("TF 1 ÷ TF 2 is a math channel: capture it to combine it with another curve")
        );
        assert_eq!(
            plan(&side("FOH", None, DropCurve::Live), &tf)
                .err()
                .as_deref(),
            Some(
                "FOH has no curve to combine: drop a live curve, a stored trace, an EQ set or a \
             target"
            )
        );
        assert_eq!(
            plan(&tf, &tf).err().as_deref(),
            Some("drop TF 1 on another curve to combine them")
        );
        assert!(swaps(&eq, &tf) && !swaps(&tf, &eq) && !swaps(&eq, &eq));
        let flat = side("flat", TF, DropCurve::Target);
        assert!(swaps(&flat, &tf) && !swaps(&flat, &eq));
    }

    #[test]
    fn the_label_names_the_channel_the_dialog_opens_with() {
        assert_eq!(
            label("TF 1 S1", MathOp::Multiply, "loudness 85 → 100 phon", false),
            "math channel: TF 1 S1 × loudness 85 → 100 phon"
        );
        assert_eq!(
            label("Main L", MathOp::Multiply, "EQ 1", true),
            "math channel: Main L after EQ 1"
        );
        assert_eq!(
            label("TF 1 S1", MathOp::Divide, "flat", false),
            "math channel: TF 1 S1 ÷ flat"
        );
    }
}
