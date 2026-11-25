//! The list of a channel field: everything the field can name, one line each, so an input
//! among many is found by reading down instead of stepping through them one at a time. A
//! rig with many inputs uses a few of them again and again: the ones the session's
//! measurements used last come first, then every one in channel order.

use super::{FieldId, Form, Value, rows};

/// The footer's cue for the list.
pub(super) const KEYS: &str = "Space lists them";
/// The footer while the list is open.
pub const OPEN_KEYS: &str = "↑↓ move · Enter or Space picks · Esc closes the list";
/// The heading of the first part.
pub const RECENT_HEADING: &str = "used last";
/// The heading of the second part, when there is a first.
pub const ALL_HEADING: &str = "all";
/// Most lines of the first part.
const RECENT: usize = 5;
/// A list shorter than this is read at a glance: no first part.
const RECENT_FROM: usize = 8;

/// One line of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    /// The channel (zero-based); `None`: the row is not used.
    pub channel: Option<u16>,
    pub name: String,
    /// In the first part: one of the channels used last.
    pub recent: bool,
}

/// An open list.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelList {
    /// The field it belongs to.
    pub field: usize,
    pub rows: Vec<ListRow>,
    pub cursor: usize,
    /// The lines are inputs (drawn with their meters), not outputs.
    pub inputs: bool,
}

fn is_output(id: FieldId) -> bool {
    matches!(id, FieldId::Output | FieldId::MoreOutput(_))
}

impl Form {
    /// What channel field `i` can name, in channel order (`None` first where the row may
    /// stay unused), each with its name. `None` when field `i` is not a channel field.
    pub fn channel_options(&self, i: usize) -> Option<Vec<(Option<u16>, String)>> {
        let f = self.fields.get(i)?;
        let Value::Channel {
            channels, options, ..
        } = &f.value
        else {
            return None;
        };
        let opts = crate::meter_dialog::bar_options(self, i)
            .or_else(|| self.row_options(i))
            .unwrap_or_else(|| channels.iter().copied().map(Some).collect());
        let none = match f.id {
            FieldId::Bar(_) => ac2_scene::programme::form::NO_BAR,
            _ => rows::NONE,
        };
        Some(
            opts.into_iter()
                .map(|o| {
                    let name = o.map_or_else(
                        || none.to_owned(),
                        |c| {
                            channels
                                .iter()
                                .position(|x| *x == c)
                                .and_then(|at| options.get(at).cloned())
                                .unwrap_or_default()
                        },
                    );
                    (o, name)
                })
                .collect(),
        )
    }

    /// Channel field `i` names `to` (one of [`Form::channel_options`]); the focus goes to it.
    pub fn set_channel_at(&mut self, i: usize, to: Option<u16>) {
        self.list = None;
        if !self
            .channel_options(i)
            .is_some_and(|o| o.iter().any(|(c, _)| *c == to))
        {
            return;
        }
        self.focus_field(i);
        if crate::meter_dialog::set_bar(self, i, to) || self.set_row(i, to) {
            return;
        }
        if let Some(Value::Channel {
            channels, index, ..
        }) = self.fields.get_mut(i).map(|f| &mut f.value)
        {
            *index = to.and_then(|c| channels.iter().position(|x| *x == c));
        }
        self.picked();
    }

    /// Opens the list of channel field `i`; `recent` are the channels of its direction the
    /// session's measurements used, the last used first. Nothing opens on any other field.
    pub fn open_list(&mut self, i: usize, recent: &[u16]) {
        let Some(opts) = self.channel_options(i) else {
            return;
        };
        if opts.is_empty() {
            return;
        }
        self.focus_field(i);
        let current = self.fields[i].channel_value();
        let mut rows: Vec<ListRow> = Vec::new();
        if opts.len() >= RECENT_FROM {
            for r in recent {
                if rows.len() < RECENT
                    && !rows.iter().any(|x| x.channel == Some(*r))
                    && let Some((c, name)) = opts.iter().find(|(c, _)| *c == Some(*r))
                {
                    rows.push(ListRow {
                        channel: *c,
                        name: name.clone(),
                        recent: true,
                    });
                }
            }
        }
        let first = rows.len();
        rows.extend(opts.into_iter().map(|(channel, name)| ListRow {
            channel,
            name,
            recent: false,
        }));
        let cursor = rows
            .iter()
            .skip(first)
            .position(|r| r.channel == current)
            .map_or(0, |at| first + at);
        self.list = Some(ChannelList {
            field: i,
            rows,
            cursor,
            inputs: !is_output(self.fields[i].id),
        });
    }

    /// ↑/↓ in the open list, stopping at its ends.
    pub fn list_move(&mut self, d: i32) {
        if let Some(l) = &mut self.list {
            let last = l.rows.len().saturating_sub(1) as i64;
            l.cursor = (l.cursor as i64 + i64::from(d)).clamp(0, last) as usize;
        }
    }

    /// Enter in the open list (`row`: none) or a click on line `row`: the field names that
    /// line's channel and the list closes.
    pub fn list_pick(&mut self, row: Option<usize>) {
        let Some(l) = self.list.take() else {
            return;
        };
        if let Some(r) = l.rows.get(row.unwrap_or(l.cursor)) {
            self.set_channel_at(l.field, r.channel);
        }
    }
}
