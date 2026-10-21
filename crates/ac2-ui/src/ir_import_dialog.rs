//! The impulse-response import dialog: a WAV / RF64 file picked by the import prompt is
//! read as an impulse response with the options of `ac2 trace import` (channel, time zero,
//! gate, resolution). The file's header is read here, client side, for its channel count:
//! a header, not samples; the response itself is the daemon's.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use ac2_proto::model::{ImportRole, IrAlign, IrImport, IrImportWindow, Resolution};

use crate::conn::Request;
use crate::forms::{Field, FieldId, Form, FormKind, Value};
use ac2_scene::unit_text::{Time, in_field};

/// What a WAV / RF64 header says about the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavHeader {
    pub channels: u16,
    pub sample_rate: u32,
    pub frames: u64,
}

/// The file the dialog imports.
#[derive(Clone, Debug, PartialEq)]
pub struct IrTarget {
    pub path: PathBuf,
    pub role: ImportRole,
    pub header: WavHeader,
    /// Where the trace is listed.
    pub owner: ac2_proto::model::TraceOwner,
}

/// Reads the header of `path`: `None` when the file does not start like a WAV / RF64 file
/// (it is imported as text, as before).
pub fn probe(path: &Path) -> Result<Option<WavHeader>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("cannot read: {e}"))?;
    read_header(&mut f)
}

/// The header of a WAV / RF64 stream: its `fmt ` chunk and the length of its `data` chunk
/// (an RF64 file's from its `ds64` chunk). Chunks between them are skipped unread.
pub fn read_header(r: &mut (impl Read + Seek)) -> Result<Option<WavHeader>, String> {
    let bad = |what: &str| format!("not a readable WAV file: {what}");
    let mut head = [0u8; 12];
    if r.read_exact(&mut head).is_err() {
        return Ok(None);
    }
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return Ok(None),
    };
    if &head[8..12] != b"WAVE" {
        return Ok(None);
    }
    let mut fmt: Option<(u16, u32, u16)> = None;
    let mut ds64_data: Option<u64> = None;
    loop {
        let mut ch = [0u8; 8];
        if r.read_exact(&mut ch).is_err() {
            return Err(bad("no audio data"));
        }
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]);
        match &ch[0..4] {
            b"fmt " => {
                let mut b = [0u8; 16];
                r.read_exact(&mut b)
                    .map_err(|_| bad("short format chunk"))?;
                let channels = u16::from_le_bytes([b[2], b[3]]);
                let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let block = u16::from_le_bytes([b[12], b[13]]);
                if channels == 0 || rate == 0 || block == 0 {
                    return Err(bad("no channels"));
                }
                fmt = Some((channels, rate, block));
                skip(r, u64::from(size).saturating_sub(16))?;
            }
            b"ds64" if rf64 => {
                let mut b = [0u8; 16];
                r.read_exact(&mut b).map_err(|_| bad("short ds64 chunk"))?;
                let mut d = [0u8; 8];
                d.copy_from_slice(&b[8..16]);
                ds64_data = Some(u64::from_le_bytes(d));
                skip(r, u64::from(size).saturating_sub(16))?;
            }
            b"data" => {
                let (channels, sample_rate, block) =
                    fmt.ok_or_else(|| bad("data before format"))?;
                let bytes = match ds64_data {
                    Some(n) if size == u32::MAX => n,
                    _ => u64::from(size),
                };
                return Ok(Some(WavHeader {
                    channels,
                    sample_rate,
                    frames: bytes / u64::from(block),
                }));
            }
            _ => skip(r, u64::from(size))?,
        }
        // Chunks are padded to an even length.
        if size % 2 == 1 {
            skip(r, 1)?;
        }
    }
}

fn skip(r: &mut impl Seek, n: u64) -> Result<(), String> {
    let n = i64::try_from(n).map_err(|_| "not a readable WAV file: chunk too long".to_string())?;
    r.seek(SeekFrom::Current(n))
        .map(|_| ())
        .map_err(|e| format!("cannot read: {e}"))
}

/// Time zero choices, index 0 the default (`ac2 trace import` without `--keep-delay`).
const TIME_ZERO: [(&str, IrAlign); 2] = [
    ("IR peak (bulk delay removed)", IrAlign::Peak),
    ("file start (keep delay)", IrAlign::Start),
];

/// One line under the Time zero choice: what the phase is referred to.
pub fn time_zero_hint(index: usize) -> &'static str {
    match TIME_ZERO.get(index).map(|t| t.1) {
        Some(IrAlign::Start) => {
            "phase includes the file's latency — use to compare against measurements timed the same way"
        }
        _ => "phase is the speaker's own; the time up to the peak becomes the trace's delay",
    }
}

const GATE_HINT: &str =
    "off: the whole file as it is; a length (ms) after the peak keeps later reflections out";
const GATE_PRE_HINT: &str =
    "with a gate: kept before the peak (ms), for pre-ringing and crossover delay";
/// `ac2 trace import --gate-pre`'s default.
const DEFAULT_GATE_PRE: &str = "1 ms";

/// The dialog for `target`: a Channel choice when the file has several, Time zero, Gate,
/// Before the peak and Resolution, each at `ac2 trace import`'s default.
pub fn form(target: IrTarget) -> Form {
    let h = target.header;
    let mut fields = Vec::new();
    if h.channels > 1 {
        let options: Vec<String> = (1..=h.channels).map(|c| c.to_string()).collect();
        let options: Vec<&str> = options.iter().map(String::as_str).collect();
        let mut c = Field::choice(FieldId::IrChannel, "Channel", &options, 0);
        c.hint = format!("{} — the impulse response is one of them", summary(&h));
        fields.push(c);
    }
    let names = TIME_ZERO.map(|t| t.0);
    let mut tz = Field::choice(FieldId::TimeZero, "Time zero", &names, 0);
    tz.hint = time_zero_hint(0).into();
    fields.push(tz);
    fields.push(Field::text(FieldId::Gate, "Gate", "off", GATE_HINT));
    fields.push(Field::text(
        FieldId::GatePre,
        "Before the peak",
        DEFAULT_GATE_PRE,
        GATE_PRE_HINT,
    ));
    fields.push(crate::forms::resolution_field(Resolution::default()));
    let mut f = Form::new(FormKind::IrImport, fields);
    f.ir_import = Some(Box::new(target));
    f
}

/// `2 channels, 48 kHz, 1.37 s`.
fn summary(h: &WavHeader) -> String {
    let secs = h.frames as f64 / f64::from(h.sample_rate);
    format!(
        "{} channels, {}, {secs:.2} s",
        h.channels,
        ac2_scene::format::freq_readout(f64::from(h.sample_rate))
    )
}

/// The import the dialog asks for, or what is wrong with it.
pub fn request(f: &Form) -> Result<Request, String> {
    let target = f
        .ir_import
        .as_ref()
        .ok_or_else(|| "not an import dialog".to_string())?;
    let channel = if target.header.channels > 1 {
        Some(f.choice_index(FieldId::IrChannel).unwrap_or(0) as u16)
    } else {
        None
    };
    let align = TIME_ZERO[f
        .choice_index(FieldId::TimeZero)
        .unwrap_or(0)
        .min(TIME_ZERO.len() - 1)]
    .1;
    let gate = f.text(FieldId::Gate).trim();
    let window = if gate.is_empty() || gate.eq_ignore_ascii_case("off") {
        IrImportWindow::Whole
    } else {
        let Time(post) = in_field(gate, "ms").map_err(|e| format!("gate: {e}"))?;
        if post.0 <= 0.0 {
            return Err("gate: a length after the peak, more than 0 ms (or off)".into());
        }
        // A duration is never negative: the parser refuses `-1 ms`.
        let Time(pre) = in_field(f.text(FieldId::GatePre), "ms")
            .map_err(|e| format!("before the peak: {e}"))?;
        IrImportWindow::Gate { pre, post }
    };
    Ok(Request::Import {
        path: target.path.clone(),
        role: target.role,
        owner: target.owner,
        ir: Some(IrImport {
            channel,
            window,
            align,
            resolution: f.resolution(),
        }),
    })
}

/// The hint of field `i` after its choice changed.
pub(crate) fn rehint(f: &mut Form, i: usize) {
    if let Some(field) = f.fields.get_mut(i)
        && field.id == FieldId::TimeZero
        && let Value::Choice { index, .. } = field.value
    {
        field.hint = time_zero_hint(index).into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header(bytes: &[u8]) -> Result<Option<WavHeader>, String> {
        read_header(&mut Cursor::new(bytes))
    }

    #[test]
    fn a_wav_header_names_its_channels_rate_and_length() {
        let b = ac2_traces::raw::wav::to_bytes(3, 44_100, &[0.0; 3 * 441]);
        assert_eq!(
            header(&b),
            Ok(Some(WavHeader {
                channels: 3,
                sample_rate: 44_100,
                frames: 441
            }))
        );
    }

    #[test]
    fn an_rf64_header_takes_its_length_from_ds64() {
        let mut b = ac2_traces::raw::wav::to_bytes(2, 48_000, &[0.0; 2 * 10]);
        // As the writer marks a file past 4 GiB: sizes in ds64, the 32-bit ones all ones.
        b[0..4].copy_from_slice(b"RF64");
        b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        b[12..16].copy_from_slice(b"ds64");
        b[28..36].copy_from_slice(&(10u64 * 8).to_le_bytes());
        b[112..116].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(header(&b).map(|h| h.map(|h| h.frames)), Ok(Some(10)));
    }

    #[test]
    fn text_is_not_a_wav_and_a_cut_header_is_unreadable() {
        assert_eq!(header(b"20 0\n20000 0\n"), Ok(None));
        let b = ac2_traces::raw::wav::to_bytes(1, 48_000, &[0.0; 4]);
        assert!(header(&b[..60]).is_err());
    }
}
