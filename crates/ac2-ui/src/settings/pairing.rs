//! The Connection page's *Paired rigs*: the rigs paired with the rig shown, each with the
//! halves the daemons' state shows, and *Pair with rig…*'s pick of a rig the app holds. The
//! rows are worked out by the reducer (`state/pairing.rs`), which sees every rig held; this
//! is what the page keeps of them between frames. Every word is `ac2_scene::pairing`'s.

use ac2_proto::units::RigId;

/// A rig paired with the rig shown, or half paired.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedRow {
    pub rig: RigId,
    pub name: String,
    pub text: String,
    /// The hover: each half in words.
    pub tip: String,
    /// Some half is seen missing: Enter pairs both ways again.
    pub mend: bool,
    /// The app holds that rig: unpairing reaches both sides.
    pub held: bool,
}

/// A rig *Pair with rig…* offers.
#[derive(Clone, Debug, PartialEq)]
pub struct PairOffer {
    pub rig: RigId,
    pub text: String,
    /// Why it cannot be paired with now (the row says it; Enter on it says it again).
    pub refusal: Option<String>,
}

/// The page's pairing state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pairing {
    pub rows: Vec<PairedRow>,
    /// The pick, while open.
    pub offers: Option<Vec<PairOffer>>,
    /// The row Delete was pressed on once: a second press unpairs.
    pub confirm: Option<RigId>,
}

impl Pairing {
    pub fn row(&self, rig: RigId) -> Option<&PairedRow> {
        self.rows.iter().find(|r| r.rig == rig)
    }

    pub fn offer(&self, rig: RigId) -> Option<&PairOffer> {
        self.offers.iter().flatten().find(|o| o.rig == rig)
    }
}
