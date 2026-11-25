//! `spl.log_get`, `spl.history_get` and `spl.band_log_get` at their documented caps fit
//! one ctrl message: a page the daemon may send is never answered `too_large` instead.

use ac2_proto::model::{LeqConfig, SplBandLog, SplHistory, SplLogPage};
use ac2_proto::samples;
use ac2_proto::units::{RequestId, WallNs};
use ac2_proto::{MAX_CTRL_BYTES, Reply, ReplyBody, encode_reply};

fn sample<T>(pick: impl Fn(ReplyBody) -> Option<T>) -> T {
    samples::replies()
        .into_iter()
        .filter_map(Result::ok)
        .find_map(pick)
        .expect("a sample reply of that kind")
}

/// Encoded size of `body` as a reply to the largest request id.
fn reply_bytes(body: ReplyBody) -> usize {
    encode_reply(&Reply::new(RequestId(u64::MAX), Ok(body)))
        .expect("encodes")
        .len()
}

fn fits(what: &str, bytes: usize) {
    assert!(
        bytes <= MAX_CTRL_BYTES,
        "{what}: {bytes} B over the {MAX_CTRL_BYTES} B ctrl limit"
    );
}

/// `spl.log_get` at [`SplLogPage::MAX_ROWS`], every row with every field present.
#[test]
fn a_full_spl_log_page_fits_a_ctrl_message() {
    let mut page = sample(|b| match b {
        ReplyBody::SplLogPage(p) => Some(p),
        _ => None,
    });
    let full = page
        .rows
        .iter()
        .find(|r| r.sensitivity.is_some() && r.position.is_some())
        .copied()
        .expect("a sample row with every field");
    page.from = u64::MAX - u64::from(SplLogPage::MAX_ROWS);
    page.total = u64::MAX;
    page.rows = (0..SplLogPage::MAX_ROWS)
        .map(|i| {
            let mut r = full;
            r.start = WallNs(u64::MAX - u64::from(i));
            r
        })
        .collect();
    fits("spl log page", reply_bytes(ReplyBody::SplLogPage(page)));
}

/// `spl.history_get` with the most windows a meter has, over the longest span either form
/// returns.
#[test]
fn a_full_spl_history_fits_a_ctrl_message() {
    let mut h: SplHistory = *sample(|b| match b {
        ReplyBody::SplHistory(h) => Some(h),
        _ => None,
    });
    let points = SplHistory::MAX_POINTS as usize;
    let window = h
        .windows
        .iter()
        .find(|w| w.limit.is_some())
        .cloned()
        .expect("a sample window with a limit");
    h.windows = vec![window; LeqConfig::MAX_WINDOWS];
    h.at = (0..points).map(|i| WallNs(u64::MAX - i as u64)).collect();
    h.leq = vec![vec![-123.456_f32; points]; LeqConfig::MAX_WINDOWS];
    h.over = vec![vec![true; points]; LeqConfig::MAX_WINDOWS];
    fits(
        "spl history",
        reply_bytes(ReplyBody::SplHistory(Box::new(h))),
    );
}

/// `spl.band_log_get` at [`SplBandLog::MAX_ROWS`], every band of every row present.
#[test]
fn a_full_band_log_fits_a_ctrl_message() {
    let mut log: SplBandLog = *sample(|b| match b {
        ReplyBody::SplBandLog(l) => Some(l),
        _ => None,
    });
    let mut row = log.rows[0];
    row.levels = [Some(-123.456_789); ac2_proto::model::BAND_COUNT];
    row.sensitivity = row.sensitivity.or(Some(ac2_proto::units::Db(120.5)));
    log.rows = (0..SplBandLog::MAX_ROWS)
        .map(|i| {
            let mut r = row;
            r.start = WallNs(u64::MAX - u64::from(i));
            r
        })
        .collect();
    fits(
        "band log",
        reply_bytes(ReplyBody::SplBandLog(Box::new(log))),
    );
}
