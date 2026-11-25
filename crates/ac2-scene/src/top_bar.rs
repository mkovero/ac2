//! The window's top bar as one line of parts: the rig shown, where its daemon runs and how
//! it is doing, then the session.
//!
//! `pupu ▾ | remote: ac2pi-0ab1c6 · 96 kHz · 256 frames · CPU 43 % · DSP 0 % ·
//! 0 xruns · 85 fps | untitled · unsaved changes`
//!
//! Plain data in, parts out: the view places them (and scrolls the line when it is wider
//! than its room), so the bar reads the same in every state the tests can name without a
//! window.

use crate::audio::StreamLine;
use crate::chrome::{LinkTone, RIG_NOT_AUTHORIZED, RigDown};
use crate::health::HealthLine;

/// The mDNS suffix a host name is shown without: `pupu.local` reads as a rig on this
/// computer, which is what `local` means in the bar.
const MDNS_SUFFIX: &str = ".local";

/// Where a rig is, as the app shows it everywhere: the host name its mDNS advert carries
/// when one was seen for it (`mdns_host`), else the `address` the operator typed. Either
/// without the `.local` suffix; a numeric address is shown as it is.
pub fn host_name(address: &str, mdns_host: Option<&str>) -> String {
    let name = mdns_host.unwrap_or(address).trim_end_matches('.');
    let cut = name.len().saturating_sub(MDNS_SUFFIX.len());
    match name.get(cut..) {
        Some(tail) if cut > 0 && tail.eq_ignore_ascii_case(MDNS_SUFFIX) => name[..cut].to_owned(),
        _ => name.to_owned(),
    }
}

/// Where the rig shown's daemon runs, as the app reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place<'a> {
    /// On this computer, over its own transport: the daemon the app starts beside itself.
    Local,
    /// Over the network, at `host` ([`host_name`]).
    Remote { host: &'a str },
}

/// The link to the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarLink<'a> {
    Connecting,
    /// The last attempt failed with `error`.
    Failed {
        error: &'a str,
        down: RigDown,
    },
    /// `responding`: its heartbeat is fresh; `synced`: the mirrored state has caught up;
    /// `view_only`: the rig authorized this app's key as a viewer.
    Connected {
        responding: bool,
        synced: bool,
        view_only: bool,
    },
}

/// The audio I/O of the rig shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarAudio<'a> {
    /// Open, on `device`.
    Open {
        device: &'a str,
        line: &'a StreamLine,
    },
    /// Open and stopped: what the bar says of it, and the banner behind it.
    Stopped { text: &'a str, hover: &'a str },
    /// None open: what is missing and the key that opens it.
    None { text: &'a str },
}

/// The working session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarSession<'a> {
    /// `friday`, `untitled · unsaved changes`.
    pub text: &'a str,
    pub unsaved: bool,
    pub detail: &'a str,
}

/// How a part is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Warn,
    Fault,
    /// The stimulus colour: a replayed recording stands in for the inputs.
    Armed,
}

/// What a part of the line stands after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lead {
    /// The line's first part.
    First,
    /// A space: it belongs to the part before it.
    Space,
    /// ` · `: another figure of the same segment.
    Dot,
    /// A rule: the next segment.
    Rule,
}

/// What a click on a part opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Settings › Connection.
    Connection,
    /// The Sessions dialog.
    Sessions,
}

/// One part of the line: a text in one colour with its own tooltip and click.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub lead: Lead,
    pub text: String,
    pub tone: Tone,
    pub hover: Option<String>,
    pub click: Option<Click>,
    /// The health meter: `text` in spans, which the view lays out in fixed slots so a
    /// figure changing moves nothing after it.
    pub figures: Option<HealthLine>,
}

impl Part {
    fn new(lead: Lead, text: impl Into<String>, tone: Tone) -> Self {
        Self {
            lead,
            text: text.into(),
            tone,
            hover: None,
            click: None,
            figures: None,
        }
    }

    fn hover(mut self, h: impl Into<String>) -> Self {
        self.hover = Some(h.into());
        self
    }

    fn click(mut self, c: Click) -> Self {
        self.click = Some(c);
        self
    }
}

/// Something more the line says after the session: an autosave, a recording, a replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub tone: Tone,
    pub hover: Option<String>,
}

/// What the bar is built from.
#[derive(Clone, Debug, PartialEq)]
pub struct BarInputs<'a> {
    /// The rig shown, as the drop-down names it.
    pub rig: &'a str,
    pub place: Place<'a>,
    /// The link's target in full, with the numeric address: for the hover.
    pub target: &'a str,
    /// The daemon's software and version, once it said hello: for the hover.
    pub server: Option<&'a str>,
    pub link: BarLink<'a>,
    pub audio: Option<BarAudio<'a>>,
    pub health: Option<HealthLine>,
    pub session: Option<BarSession<'a>>,
    pub notes: Vec<Note>,
}

/// The bar: the rig shown with its link's tone, fixed at the left, and the line after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub rig: String,
    /// The dot on the rig's name.
    pub tone: LinkTone,
    pub line: Vec<Part>,
}

impl Bar {
    /// The line as one string, a rule as ` | `.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for p in &self.line {
            s.push_str(match p.lead {
                Lead::First => "",
                Lead::Space => " ",
                Lead::Dot => " · ",
                Lead::Rule => " | ",
            });
            s.push_str(&p.text);
        }
        s
    }
}

/// What the bar says after the place of a rig that authorized this app as a viewer.
pub const VIEW_ONLY: &str = "view only";

/// Its hover, and what an edit refused on such a rig says.
pub const VIEW_ONLY_DETAIL: &str = "This rig authorized this app as a viewer: it shows \
     everything and may stop the generator, and every change is refused. An operator of the \
     rig changes the role (Settings › Connection, or ac2 auth role).";

/// `local`, `remote: ac2pi-0ab1c6`; a host called as the rig is not said twice: `remote`.
fn place_text(rig: &str, place: Place<'_>) -> String {
    match place {
        Place::Local => "local".to_owned(),
        Place::Remote { host } if host.eq_ignore_ascii_case(rig) => "remote".to_owned(),
        Place::Remote { host } => format!("remote: {host}"),
    }
}

/// The bar for `i`.
pub fn bar(i: &BarInputs<'_>) -> Bar {
    let place = place_text(i.rig, i.place);
    // The software and the numeric address are for whoever asks: the hover and Settings.
    let whole = match i.server {
        Some(s) => format!("{s} · {}", i.target),
        None => i.target.to_owned(),
    };
    let about = whole;
    // Link trouble takes the daemon's segment over: nothing it would say after is current.
    let trouble = match i.link {
        BarLink::Connecting => {
            let whom = match i.place {
                Place::Local => "the local daemon",
                Place::Remote { host } => host,
            };
            Some((LinkTone::Warn, format!("connecting to {whom}…"), None))
        }
        BarLink::Failed {
            error,
            down: RigDown::NotAuthorized,
        } => Some((
            LinkTone::Fault,
            format!("{place} · {RIG_NOT_AUTHORIZED}"),
            Some(error),
        )),
        BarLink::Failed {
            down: RigDown::NetworkOff,
            ..
        } => Some((
            LinkTone::Warn,
            format!("{place} · {}", crate::network::RIG_NETWORK_OFF),
            None,
        )),
        BarLink::Failed {
            error,
            down: RigDown::Unreachable,
        } => Some((
            LinkTone::Fault,
            format!("{place} · {error} · retrying"),
            None,
        )),
        BarLink::Connected {
            responding: false, ..
        } => Some((LinkTone::Fault, format!("{place} · not responding"), None)),
        BarLink::Connected { synced: false, .. } => {
            Some((LinkTone::Warn, format!("{place} · syncing"), None))
        }
        BarLink::Connected { .. } => None,
    };
    let mut line = Vec::new();
    let tone = match trouble {
        Some((tone, text, more)) => {
            let t = match tone {
                LinkTone::Fault => Tone::Fault,
                _ => Tone::Warn,
            };
            let hover = match more {
                Some(m) => format!("{m}\n{about}"),
                None => about,
            };
            line.push(
                Part::new(Lead::First, text, t)
                    .hover(hover)
                    .click(Click::Connection),
            );
            tone
        }
        None => {
            let (place, hover) = match i.link {
                BarLink::Connected {
                    view_only: true, ..
                } => (
                    format!("{place} · {VIEW_ONLY}"),
                    format!("{VIEW_ONLY_DETAIL}\n{about}"),
                ),
                _ => (place, about),
            };
            line.push(
                Part::new(Lead::First, place, Tone::Plain)
                    .hover(hover)
                    .click(Click::Connection),
            );
            daemon_parts(i, &mut line);
            LinkTone::Ok
        }
    };
    if let Some(s) = i.session {
        let tone = if s.unsaved { Tone::Warn } else { Tone::Plain };
        line.push(
            Part::new(Lead::Rule, s.text, tone)
                .hover(s.detail)
                .click(Click::Sessions),
        );
        for n in &i.notes {
            let mut p = Part::new(Lead::Dot, n.text.clone(), n.tone);
            p.hover.clone_from(&n.hover);
            line.push(p);
        }
    }
    Bar {
        rig: i.rig.to_owned(),
        tone,
        line,
    }
}

/// What a daemon whose link is fine says after its place: the audio I/O and the health
/// meter. The stimulus is not on the line: it is
/// fixed at the bar's right end ([`crate::stim_item`]).
fn daemon_parts(i: &BarInputs<'_>, line: &mut Vec<Part>) {
    match i.audio {
        Some(BarAudio::Open { device, line: l }) => {
            let tone = if l.changed { Tone::Warn } else { Tone::Dim };
            let text = match &l.change {
                Some(c) => format!("{} · {c}", l.short),
                None => l.short.clone(),
            };
            let hover = match &l.detail {
                Some(d) => format!("{device} · {d}"),
                None => device.to_owned(),
            };
            line.push(Part::new(Lead::Dot, text, tone).hover(hover));
        }
        Some(BarAudio::Stopped { text, hover }) => {
            line.push(Part::new(Lead::Dot, text, Tone::Fault).hover(hover));
        }
        Some(BarAudio::None { text }) => line.push(Part::new(Lead::Dot, text, Tone::Dim)),
        None => {}
    }
    if let Some(h) = &i.health {
        let mut p = Part::new(Lead::Dot, h.text(), Tone::Dim).hover(h.detail.clone());
        p.figures = Some(h.clone());
        line.push(p);
    }
}

#[cfg(test)]
mod tests;
