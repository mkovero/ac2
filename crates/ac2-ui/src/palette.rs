//! Command palette: fuzzy matching over every command's title and name.

use crate::keys::{CommandId, Keymap, Scope};

/// A match of `query` in `text`: score (higher is better) and the matched char indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

fn is_boundary(prev: Option<char>) -> bool {
    prev.is_none_or(|p| !p.is_alphanumeric())
}

/// Fuzzy subsequence match, case-insensitive. Every query character (spaces ignored) must
/// appear in order. Rewards consecutive runs, word starts and an early first match;
/// penalises gaps. Greedy from each possible start of the first character, best kept.
pub fn fuzzy(query: &str, text: &str) -> Option<Match> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(Match {
            score: 0,
            positions: vec![],
        });
    }
    let t: Vec<char> = text.chars().collect();
    let lower: Vec<char> = t
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut best: Option<Match> = None;
    for start in (0..lower.len()).filter(|&i| lower[i] == q[0]) {
        let mut positions = vec![start];
        let mut i = start + 1;
        for &qc in &q[1..] {
            // Prefer the next word start holding qc when the immediate char does not match.
            let next = (i..lower.len()).find(|&j| lower[j] == qc);
            let Some(mut j) = next else {
                positions.clear();
                break;
            };
            if j != i
                && let Some(w) = (j..lower.len())
                    .find(|&k| lower[k] == qc && is_boundary(k.checked_sub(1).map(|p| t[p])))
            {
                j = w;
            }
            positions.push(j);
            i = j + 1;
        }
        if positions.len() != q.len() {
            continue;
        }
        let mut score = 0i32;
        for (n, &p) in positions.iter().enumerate() {
            score += 10;
            if is_boundary(p.checked_sub(1).map(|x| t[x])) {
                score += 15;
            }
            if n > 0 {
                let gap = p - positions[n - 1] - 1;
                if gap == 0 {
                    score += 12;
                } else {
                    score -= gap.min(10) as i32;
                }
            }
        }
        score -= positions[0].min(20) as i32;
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Match { score, positions });
        }
    }
    best
}

/// One palette row.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub command: CommandId,
    pub title: &'static str,
    /// Bound key(s) as shown, e.g. `X` or `H (spectrum)`.
    pub key: Option<String>,
    /// Matched char indices in `title`.
    pub positions: Vec<usize>,
}

/// Commands matching `query`, best first; all commands in table order for an empty query.
pub fn search(query: &str, keymap: &Keymap, active: Scope) -> Vec<Entry> {
    let mut rows: Vec<(i32, usize, Entry)> = CommandId::ALL
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| {
            let title = c.title();
            let by_title = fuzzy(query, title);
            // The snake_case name (what keys.toml uses) matches too, without highlighting.
            let by_name = fuzzy(query, c.name()).map(|m| Match {
                score: m.score - 5,
                positions: vec![],
            });
            let m = match (by_title, by_name) {
                (Some(a), Some(b)) => Some(if b.score > a.score { b } else { a }),
                (a, b) => a.or(b),
            }?;
            // Commands that act in the focused pane rank above the others.
            let here = c.scopes().contains(&active) || c.scopes().contains(&Scope::Global);
            Some((
                m.score + if here { 3 } else { 0 },
                i,
                Entry {
                    command: c,
                    title,
                    key: keymap.key_hint(c, active),
                    positions: m.positions,
                },
            ))
        })
        .collect();
    if !query.trim().is_empty() {
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    rows.into_iter().map(|r| r.2).collect()
}

/// Commands the palette shows at once; PageUp / PageDown move by this many.
pub const PALETTE_ROWS: usize = 12;

/// Palette state: the query and the highlighted row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
}

impl Palette {
    pub fn entries(&self, keymap: &Keymap, active: Scope) -> Vec<Entry> {
        search(&self.query, keymap, active)
    }

    pub fn type_text(&mut self, s: &str) {
        self.query.push_str(s);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Moves the highlight by `d` rows, clamped to `n` rows.
    pub fn move_by(&mut self, d: i32, n: usize) {
        if n == 0 {
            self.selected = 0;
            return;
        }
        let s = self.selected as i64 + i64::from(d);
        self.selected = s.clamp(0, n as i64 - 1) as usize;
    }

    /// The command Enter would run.
    pub fn chosen(&self, keymap: &Keymap, active: Scope) -> Option<CommandId> {
        self.entries(keymap, active)
            .get(self.selected)
            .map(|e| e.command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_and_case() {
        assert!(fuzzy("gd", "Phase / group delay").is_some());
        assert!(fuzzy("GROUP", "Phase / group delay").is_some());
        assert!(fuzzy("xyz", "Phase / group delay").is_none());
        assert!(fuzzy("dg", "group delay").is_none());
        assert_eq!(fuzzy("", "anything").map(|m| m.score), Some(0));
    }

    #[test]
    fn word_starts_and_runs_win() {
        let a = fuzzy("gd", "Phase / group delay").expect("match");
        // g of "group", d of "delay" (a word start), not the d inside some word.
        assert_eq!(a.positions, vec![8, 14]);
        let run = fuzzy("del", "Delay tracking").expect("match");
        let scattered = fuzzy("del", "Show / hide IR pane, double click").map(|m| m.score);
        assert!(scattered.is_none_or(|s| run.score > s));
    }

    #[test]
    fn search_ranks_the_obvious_command_first() {
        let k = Keymap::default();
        for (q, want) in [
            ("insert", CommandId::InsertDelay),
            ("group", CommandId::GroupDelay),
            ("theme", CommandId::CycleTheme),
            ("quit", CommandId::Quit),
            ("track", CommandId::TrackDelay),
            ("peak", CommandId::PeakHold),
            ("cohmask", CommandId::CoherenceMask),
            ("ir mode", CommandId::IrMode),
            ("stim stop", CommandId::StimulusStop),
            ("type level", CommandId::StimulusLevel),
            ("slot 3", CommandId::Slot3),
            ("open session", CommandId::OpenSession),
            ("close session", CommandId::CloseSession),
            ("record", CommandId::Record),
            ("replay", CommandId::ReplayRecording),
            ("new transfer", CommandId::NewTransfer),
            ("new rta", CommandId::NewRta),
            ("new spl", CommandId::NewSpl),
            ("delete meas", CommandId::DeleteMeasurement),
            ("leq limits", CommandId::LeqWindows),
            ("leq columns", CommandId::SplLeqStyle),
            ("leq tiles", CommandId::SplLeqStyle),
            ("leq history", CommandId::SplLeqHistory),
            ("meter + leq", CommandId::SplShowMeterLeq),
            ("pane: the meter", CommandId::SplShowMeter),
            ("pane: the leq", CommandId::SplShowLeq),
            ("new log", CommandId::SplNewLog),
            ("full screen", CommandId::Fullscreen),
            ("key hints", CommandId::KeyHints),
        ] {
            let r = search(q, &k, Scope::Transfer);
            assert_eq!(r.first().map(|e| e.command), Some(want), "{q}: {r:?}");
        }
    }

    #[test]
    fn rows_show_keys() {
        let k = Keymap::default();
        let r = search("", &k, Scope::Spectrum);
        assert_eq!(r.len(), CommandId::ALL.len());
        let row = |c| r.iter().find(|e| e.command == c).expect("row");
        assert_eq!(row(CommandId::PeakHold).key.as_deref(), Some("P"));
        assert_eq!(
            row(CommandId::InsertDelay).key.as_deref(),
            Some("X (transfer)")
        );
        assert_eq!(row(CommandId::Reconnect).key, None);
    }

    #[test]
    fn palette_navigation() {
        let k = Keymap::default();
        let mut p = Palette::default();
        p.type_text("zoom");
        let n = p.entries(&k, Scope::Global).len();
        assert!(n >= 2);
        p.move_by(1, n);
        let second = p.chosen(&k, Scope::Global);
        p.move_by(100, n);
        assert_eq!(p.selected, n - 1);
        p.move_by(-100, n);
        assert_eq!(p.selected, 0);
        assert_ne!(p.chosen(&k, Scope::Global), second);
        p.backspace();
        assert_eq!(p.query, "zoo");
    }
}
