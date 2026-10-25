//! Reducer tests of input kinds and stereo pairs in the Audio I/O page: picking two inputs
//! makes a pair, the refusals keep the pick, and kinds and pairs are remembered per device.

use super::*;

fn opened_pairs(r: &[Request]) -> Option<&[Pair]> {
    r.iter().find_map(|r| match r {
        Request::OpenSession { pairs, .. } => Some(pairs.as_slice()),
        _ => None,
    })
}

/// L and A set an input's kind, Shift+P on two inputs pairs them, N names the pair; the
/// refusals name the rule, and the opened session carries the kinds and the pair.
#[test]
fn kinds_and_pairs_by_key() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    // A never-declared row is a line input; the remembered mic stays a mic.
    let d = dialog(&t);
    assert_eq!(d.inputs[1].kind_pref(), KindPref::Mic);
    assert_eq!(d.inputs[2].kind_pref(), KindPref::Line);
    assert!(!d.inputs[2].in_session);
    let notice = |t: &T| dialog(t).notice.clone().unwrap_or_default();
    focus(&mut t, Row::Input(2));
    t.type_key("Space", " ");
    // A makes in 4 digital; a line and a digital input are not a pair.
    focus(&mut t, Row::Input(3));
    t.type_key("Space", " ");
    t.key("A");
    assert_eq!(dialog(&t).inputs[3].kind_pref(), KindPref::Digital);
    focus(&mut t, Row::Input(2));
    t.type_key("Shift+P", "P");
    assert_eq!(
        notice(&t),
        "in 3 picked: pair on the other input (again on in 3 cancels)"
    );
    focus(&mut t, Row::Input(3));
    t.type_key("Shift+P", "P");
    assert_eq!(
        notice(&t),
        "in 3 is line and in 4 is digital: a pair is two of one kind (L or A on the rows); \
         in 3 stays picked"
    );
    assert_eq!(dialog(&t).pair_pick, Some(2));
    // Both digital: paired.
    focus(&mut t, Row::Input(2));
    t.key("A");
    focus(&mut t, Row::Input(3));
    t.type_key("Shift+P", "P");
    assert_eq!(notice(&t), "in 3 and 4 are a pair (N names it)");
    assert_eq!(dialog(&t).pair_pick, None);
    focus(&mut t, Row::Input(2));
    assert_eq!(
        dialog(&t).pairs,
        vec![Pair {
            left: 2,
            right: 3,
            name: None
        }]
    );
    // L on a paired row changes both rows; the pair stays.
    t.key("L");
    assert_eq!(dialog(&t).inputs[2].kind_pref(), KindPref::Line);
    assert_eq!(dialog(&t).inputs[3].kind_pref(), KindPref::Line);
    assert_eq!(dialog(&t).pairs.len(), 1);
    // N on a paired row names the pair.
    t.type_key("N", "n");
    assert_eq!(dialog(&t).edit, Some(Edit::PairName(2)));
    t.text("Main");
    t.key("Enter");
    assert_eq!(dialog(&t).pairs[0].name.as_deref(), Some("Main"));
    assert_eq!(dialog(&t).edit, None);
    // M on a paired row: a mic, and the pair is gone.
    focus(&mut t, Row::Input(3));
    t.key("M");
    assert_eq!(
        notice(&t),
        "in 3 and 4 are no longer a pair: a pair is two line or two digital inputs"
    );
    assert!(dialog(&t).pairs.is_empty());
    assert_eq!(dialog(&t).inputs[3].kind_pref(), KindPref::Mic);
    // Shift+P onto a mic is refused by the rule.
    focus(&mut t, Row::Input(3));
    t.type_key("Shift+P", "P");
    assert_eq!(
        notice(&t),
        "in 4 is a mic: a pair is two line or two digital inputs (L or A on the rows)"
    );
    assert_eq!(dialog(&t).pair_pick, None);
    // Line again, paired, opened: the setups carry the kinds and the sides, the session
    // the pair, and the device remembers both.
    t.key("L");
    focus(&mut t, Row::Input(2));
    t.type_key("Shift+P", "P");
    focus(&mut t, Row::Input(3));
    t.type_key("Shift+P", "P");
    let r = t.key("Enter");
    let (config, inputs) = opened(&r).expect("open");
    assert_eq!(config.input_channels, vec![0, 1, 2, 3]);
    let setup = |c: u16| inputs.iter().find(|i| i.channel == c).expect("setup");
    assert_eq!(setup(2).kind, InputKind::Line);
    assert_eq!(setup(2).pair, Some(PairSide::Left));
    assert_eq!(setup(3).pair, Some(PairSide::Right));
    assert_eq!(setup(1).pair, None);
    let pair = Pair {
        left: 2,
        right: 3,
        name: None,
    };
    assert_eq!(opened_pairs(&r), Some(std::slice::from_ref(&pair)));
    let roles = &t.st.prefs.sessions["fake/fake:loop"];
    assert_eq!(roles.kinds.get(&2), Some(&KindPref::Line));
    assert_eq!(roles.kinds.get(&3), Some(&KindPref::Line));
    assert_eq!(roles.pairs, vec![pair]);
}

/// *pair* clicked on one input.
fn click_pair(t: &mut T, i: usize) {
    t.st.update(
        Msg::Session(SessionMsg::Role(Row::Input(i), RoleKey::Pair)),
        &t.keys,
    );
}

/// The one pair of the dialog as (left, right).
fn the_pair(t: &T) -> Option<(u16, u16)> {
    match dialog(t).pairs.as_slice() {
        [p] => Some((p.left, p.right)),
        [] => None,
        more => panic!("{more:?}"),
    }
}

/// *pair* on in 1 then on in 4 pairs them across the mic between: adjacency does not matter,
/// and both go in the session without Space.
#[test]
fn pair_by_two_clicks_any_inputs() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    assert!(!dialog(&t).inputs[3].in_session);
    click_pair(&mut t, 0);
    assert_eq!(dialog(&t).pair_pick, Some(0));
    assert_eq!(the_pair(&t), None);
    click_pair(&mut t, 3);
    assert_eq!(the_pair(&t), Some((0, 3)));
    assert_eq!(dialog(&t).pair_pick, None);
    assert!(dialog(&t).inputs[0].in_session && dialog(&t).inputs[3].in_session);
    assert_eq!(
        dialog(&t).notice.as_deref(),
        Some("in 1 and 4 are a pair (N names it)")
    );
}

/// The higher input clicked first still ends up right.
#[test]
fn pair_left_is_the_lower_input() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    click_pair(&mut t, 3);
    click_pair(&mut t, 2);
    assert_eq!(the_pair(&t), Some((2, 3)));
    assert!(dialog(&t).inputs[2].in_session && dialog(&t).inputs[3].in_session);
}

/// *pair* again on the picked row drops the pick; Esc closes the dialog as always, the
/// pick with it.
#[test]
fn pair_pick_cancels() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    click_pair(&mut t, 2);
    click_pair(&mut t, 2);
    assert_eq!(dialog(&t).pair_pick, None);
    assert_eq!(
        dialog(&t).notice.as_deref(),
        Some("in 3 no longer picked for a pair")
    );
    focus(&mut t, Row::Input(2));
    t.type_key("Shift+P", "P");
    assert_eq!(dialog(&t).pair_pick, Some(2));
    t.key("Escape");
    assert_eq!(t.st.overlay, Overlay::None);
    open_dialog(&mut t, backends(true));
    assert_eq!(dialog(&t).pair_pick, None);
    assert_eq!(the_pair(&t), None);
}

/// A mic is never paired: a first pick on it is refused, a second keeps the first pick for
/// another try; mixed kinds likewise, and no kind changes.
#[test]
fn pair_refusals_keep_the_first_pick() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    let notice = |t: &T| dialog(t).notice.clone().unwrap_or_default();
    click_pair(&mut t, 1);
    assert_eq!(
        notice(&t),
        "in 2 is a mic: a pair is two line or two digital inputs (L or A on the rows)"
    );
    assert_eq!(dialog(&t).pair_pick, None);
    click_pair(&mut t, 2);
    click_pair(&mut t, 1);
    assert_eq!(
        notice(&t),
        "in 2 is a mic: a pair is two line or two digital inputs (L or A on the rows); \
         in 3 stays picked"
    );
    assert_eq!(dialog(&t).pair_pick, Some(2));
    assert_eq!(dialog(&t).inputs[1].kind_pref(), KindPref::Mic);
    // Mixed kinds: refused, kinds kept, pick kept.
    focus(&mut t, Row::Input(3));
    t.key("A");
    click_pair(&mut t, 3);
    assert_eq!(
        notice(&t),
        "in 3 is line and in 4 is digital: a pair is two of one kind (L or A on the rows); \
         in 3 stays picked"
    );
    assert_eq!(dialog(&t).inputs[2].kind_pref(), KindPref::Line);
    assert_eq!(dialog(&t).inputs[3].kind_pref(), KindPref::Digital);
    assert_eq!(dialog(&t).pair_pick, Some(2));
    // Another line input completes the pair with the kept pick.
    click_pair(&mut t, 0);
    assert_eq!(the_pair(&t), Some((0, 2)));
}

/// *pair* on either row of a pair unpairs it; the pick is dropped when its row leaves the
/// session or becomes a mic.
#[test]
fn pair_unpairs_and_pick_follows_the_row() {
    let mut t = T::new();
    open_dialog(&mut t, backends(true));
    click_pair(&mut t, 2);
    click_pair(&mut t, 3);
    click_pair(&mut t, 3);
    assert_eq!(the_pair(&t), None);
    assert_eq!(
        dialog(&t).notice.as_deref(),
        Some("in 3 and 4 are no longer a pair")
    );
    // Picked, then out of the session (in 3 went in with the pair): no pick.
    click_pair(&mut t, 2);
    focus(&mut t, Row::Input(2));
    t.type_key("Space", " ");
    assert!(!dialog(&t).inputs[2].in_session);
    assert_eq!(dialog(&t).pair_pick, None);
    // Picked, then a mic: no pick.
    click_pair(&mut t, 2);
    t.key("M");
    assert_eq!(dialog(&t).pair_pick, None);
}

/// The remembered kinds and pair of a device come back in the dialog and open with it; a
/// pair whose input is out of the session is dropped, not sent.
#[test]
fn kinds_and_pairs_are_remembered_per_device() {
    let mut t = T::new();
    let pair = Pair {
        left: 2,
        right: 3,
        name: Some("AES".into()),
    };
    let roles = crate::prefs::DeviceRoles {
        inputs: vec![0, 1, 2, 3],
        outputs: 2,
        reference: Some(0),
        mics: vec![1],
        stimulus: vec![0],
        mic_names: [(1, "M30".to_owned())].into(),
        kinds: [
            (0, KindPref::Line),
            (1, KindPref::Mic),
            (2, KindPref::Digital),
            (3, KindPref::Digital),
        ]
        .into(),
        pairs: vec![pair.clone()],
        output_device: None,
    };
    t.st.prefs
        .sessions
        .insert("fake/fake:loop".into(), roles.clone());
    open_dialog(&mut t, backends(true));
    let d = dialog(&t);
    assert_eq!(d.inputs[2].kind_pref(), KindPref::Digital);
    assert_eq!(d.inputs[3].kind_pref(), KindPref::Digital);
    assert_eq!(
        d.pair_of(2).map(|(p, s)| (p.clone(), s)),
        Some((pair.clone(), PairSide::Left))
    );
    assert_eq!(d.pair_of(3).map(|(_, s)| s), Some(PairSide::Right));
    let r = t.key("Enter");
    assert_eq!(opened_pairs(&r), Some(std::slice::from_ref(&pair)));
    // One input of the pair out of the session, or a hand-edited file pairing two kinds:
    // no pair.
    for edit in [
        |r: &mut crate::prefs::DeviceRoles| r.inputs = vec![0, 1, 2],
        |r: &mut crate::prefs::DeviceRoles| {
            r.kinds.insert(3, KindPref::Line);
        },
    ] {
        let mut roles = roles.clone();
        edit(&mut roles);
        t.st.prefs.sessions.insert("fake/fake:loop".into(), roles);
        open_dialog(&mut t, backends(true));
        assert!(dialog(&t).pairs.is_empty());
        let r = t.key("Enter");
        assert_eq!(opened_pairs(&r), Some(&[][..]));
    }
}

/// The footer of the Audio I/O page names the kind and pair keys in the row's words.
#[test]
fn audio_io_key_tips_name_the_kind_and_pair_keys() {
    use crate::keys::LabelStyle;
    let tips = crate::settings::Page::AudioIo.keys_in(LabelStyle::Pc);
    assert!(
        tips.contains("M mic · L line · A digital · Shift+P pair · N name"),
        "{tips}"
    );
    let tips = crate::settings::Page::AudioIo.keys_in(LabelStyle::Mac);
    assert!(tips.contains("⇧P pair"), "{tips}");
}
