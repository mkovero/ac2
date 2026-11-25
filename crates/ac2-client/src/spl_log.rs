//! An SPL meter's whole per-second log read from a daemon, a page of `spl.log_get` at a
//! time, and written as the log CSV (`docs/protocol.md` §7.4). Used by the CLI and the app
//! alike, so both export the same file.

use ac2_proto::model::{MeasKind, Measurement, SplLogPage, SplLogRow, SplLogWhich, State};
use ac2_proto::spl_log::SplLogInfo;
use ac2_proto::units::{MeasId, WallNs};
use ac2_proto::{Command, ReplyBody};

use crate::{Client, ClientError, expect_body};

/// One of a meter's logs as read: its start and its rows, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct LogDownload {
    /// The log's `started_at`: its identity for a client's cache.
    pub started_at: WallNs,
    /// Every row, oldest first.
    pub rows: Vec<SplLogRow>,
}

/// Every row of one of meter `meas`'s logs, oldest first: pages of
/// [`SplLogPage::MAX_ROWS`] from row 0 (the daemon starts a page at its oldest kept row when
/// older ones were dropped) until a page ends at the log's total. A page of another
/// `started_at` than the first (the log replaced, or started anew after a fault while
/// being read) starts the download over, so rows of two logs never mix.
pub async fn all_rows(
    c: &Client,
    meas: MeasId,
    log: SplLogWhich,
) -> Result<LogDownload, ClientError> {
    let mut rows = Vec::new();
    let mut from = 0;
    let mut started_at = None;
    loop {
        let r = c
            .call(Command::SplLogGet {
                meas,
                log,
                from,
                max: SplLogPage::MAX_ROWS,
            })
            .await?;
        let p = expect_body!("spl.log_get", r, ReplyBody::SplLogPage(p) => p)?;
        if started_at.is_some_and(|s| s != p.started_at) {
            rows.clear();
            from = 0;
            started_at = None;
            continue;
        }
        started_at = Some(p.started_at);
        let n = p.rows.len() as u64;
        rows.extend(p.rows);
        from = p.from + n;
        if n == 0 || from >= p.total {
            return Ok(LogDownload {
                started_at: p.started_at,
                rows,
            });
        }
    }
}

/// The CSV header's facts of SPL meter `m`: its id, name, input, that input's mic, the
/// rig's zone, and `started`, the log's `started_at` as its download read it
/// ([`LogDownload::started_at`]); `None` when `m` is not an SPL meter.
pub fn log_info(st: &State, m: &Measurement, started: WallNs) -> Option<SplLogInfo> {
    let MeasKind::Spl { config } = &m.config.kind else {
        return None;
    };
    Some(SplLogInfo {
        meas: m.id,
        name: m.config.name.clone(),
        input: config.input,
        mic: st
            .inputs
            .iter()
            .find(|i| i.channel == config.input)
            .and_then(|i| i.kind.mic().map(str::to_owned)),
        zone: st.rig.zone.label(),
        started,
    })
}
