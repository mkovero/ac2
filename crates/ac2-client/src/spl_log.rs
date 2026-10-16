//! An SPL meter's whole per-second log read from a daemon, a page of `spl.log_get` at a
//! time, and written as the log CSV (`docs/protocol.md` §7.4). Used by the CLI and the app
//! alike, so both export the same file.

use ac2_proto::model::{MeasKind, Measurement, SplLogPage, SplLogRow, SplLogWhich, State};
use ac2_proto::spl_log::SplLogInfo;
use ac2_proto::units::MeasId;
use ac2_proto::{Command, ReplyBody};

use crate::{Client, ClientError, expect_body};

/// Every row of one of meter `meas`'s logs, oldest first: pages of
/// [`SplLogPage::MAX_ROWS`] from row 0 (the daemon starts a page at its oldest kept row when
/// older ones were dropped) until a page ends at the log's total.
pub async fn all_rows(
    c: &Client,
    meas: MeasId,
    log: SplLogWhich,
) -> Result<Vec<SplLogRow>, ClientError> {
    let mut rows = Vec::new();
    let mut from = 0;
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
        let n = p.rows.len() as u64;
        rows.extend(p.rows);
        from = p.from + n;
        if n == 0 || from >= p.total {
            return Ok(rows);
        }
    }
}

/// The CSV header's facts of SPL meter `m`: its id, name, input and that input's mic;
/// `None` when `m` is not an SPL meter.
pub fn log_info(st: &State, m: &Measurement) -> Option<SplLogInfo> {
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
            .and_then(|i| i.mic.clone()),
    })
}
