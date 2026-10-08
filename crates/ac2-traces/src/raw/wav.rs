//! 32-bit float WAV that grows into RF64 past 4 GiB (EBU Tech 3306), written and read
//! without a third-party codec.
//!
//! The header has a fixed layout so that finishing a file only patches sizes in place:
//!
//! ```text
//!   0 "RIFF" | "RF64"   u32 RIFF size (0xFFFF_FFFF in RF64)   "WAVE"
//!  12 "JUNK" | "ds64"   u32 28   RIFF size u64, data size u64, sample count u64, table 0
//!  48 "fmt "            u32 40   WAVE_FORMAT_EXTENSIBLE, IEEE float subformat, 32 bits
//!  96 "fact"            u32 4    frames (0xFFFF_FFFF in RF64)
//! 108 "data"            u32 data size (0xFFFF_FFFF in RF64)
//! 116 samples, little-endian f32, interleaved
//! ```
//!
//! The `JUNK` chunk reserves the room the `ds64` chunk needs, so a file that crosses the
//! 32-bit size limit becomes RF64 by rewriting those 48 bytes, never by moving the audio.
//! Float samples carry the captured values exactly: what the converter delivered is what a
//! replay feeds the analyses. The extensible format (rather than plain format tag 3) is the
//! one readers expect for more than two channels; the channel mask is 0 because measurement
//! inputs have no loudspeaker positions.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use thiserror::Error;

/// Bytes before the first sample.
pub const HEADER_BYTES: u64 = 116;

/// Bytes per sample.
const SAMPLE_BYTES: u64 = 4;

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, as stored (little-endian GUID fields).
const FLOAT_GUID: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

/// Largest RIFF size a plain WAV header can state.
const RIFF_LIMIT: u64 = u32::MAX as u64;

/// What a WAV file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    /// Sample rate, Hz.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Frames of audio.
    pub frames: u64,
}

/// An unreadable or unusable audio file.
#[derive(Debug, Error)]
pub enum WavError {
    /// The file system refused.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// Not a RIFF/RF64 WAVE file.
    #[error("not a WAV file")]
    NotWav,
    /// A WAVE file this build does not replay (only 32-bit float is recorded).
    #[error("unsupported WAV format: {0}")]
    Format(String),
    /// Malformed chunks.
    #[error("malformed WAV file: {0}")]
    Malformed(String),
}

fn header(channels: u16, sample_rate: u32, frames: u64, rf64_above: u64) -> [u8; 116] {
    let block = u64::from(channels) * SAMPLE_BYTES;
    let data = frames * block;
    let riff = HEADER_BYTES - 8 + data;
    let rf64 = riff > rf64_above;
    let mut h = [0u8; 116];
    let mut put = |at: usize, b: &[u8]| h[at..at + b.len()].copy_from_slice(b);
    let small = |v: u64| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
    if rf64 {
        put(0, b"RF64");
        put(4, &u32::MAX.to_le_bytes());
    } else {
        put(0, b"RIFF");
        put(4, &small(riff));
    }
    put(8, b"WAVE");
    put(12, if rf64 { b"ds64" } else { b"JUNK" });
    put(16, &28u32.to_le_bytes());
    if rf64 {
        put(20, &riff.to_le_bytes());
        put(28, &data.to_le_bytes());
        put(36, &frames.to_le_bytes());
        // 44: table length 0.
    }
    put(48, b"fmt ");
    put(52, &40u32.to_le_bytes());
    put(56, &FORMAT_EXTENSIBLE.to_le_bytes());
    put(58, &channels.to_le_bytes());
    put(60, &sample_rate.to_le_bytes());
    put(64, &small(u64::from(sample_rate) * block));
    put(68, &u16::try_from(block).unwrap_or(u16::MAX).to_le_bytes());
    put(70, &32u16.to_le_bytes());
    put(72, &22u16.to_le_bytes());
    put(74, &32u16.to_le_bytes());
    // 76: channel mask 0.
    put(80, &FLOAT_GUID);
    put(96, b"fact");
    put(100, &4u32.to_le_bytes());
    if rf64 {
        put(104, &u32::MAX.to_le_bytes());
    } else {
        put(104, &small(frames));
    }
    put(108, b"data");
    if rf64 {
        put(112, &u32::MAX.to_le_bytes());
    } else {
        put(112, &small(data));
    }
    h
}

/// Appends interleaved `f32` frames to a new file and finishes its header.
#[derive(Debug)]
pub struct WavWriter {
    out: BufWriter<File>,
    channels: u16,
    sample_rate: u32,
    frames: u64,
    bytes: Vec<u8>,
    rf64_above: u64,
}

/// Write buffer: large enough that a slow card sees few, long writes.
const WRITE_BUFFER: usize = 1 << 18;

impl WavWriter {
    /// Creates `path` (never replacing an existing file) for `channels` at `sample_rate`.
    pub fn create(path: &Path, channels: u16, sample_rate: u32) -> io::Result<Self> {
        Self::create_with_limit(path, channels, sample_rate, RIFF_LIMIT)
    }

    fn create_with_limit(
        path: &Path,
        channels: u16,
        sample_rate: u32,
        rf64_above: u64,
    ) -> io::Result<Self> {
        if channels == 0 || u64::from(channels) * SAMPLE_BYTES > u64::from(u16::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{channels} channels cannot be stored in one WAV file"),
            ));
        }
        let f = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut out = BufWriter::with_capacity(WRITE_BUFFER, f);
        out.write_all(&header(channels, sample_rate, 0, rf64_above))?;
        Ok(Self {
            out,
            channels,
            sample_rate,
            frames: 0,
            bytes: Vec::new(),
            rf64_above,
        })
    }

    /// Interleaved channels.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Frames written.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Size of the file once finished, bytes.
    pub fn bytes(&self) -> u64 {
        file_bytes(self.channels, self.frames)
    }

    /// Appends whole interleaved frames.
    pub fn write(&mut self, interleaved: &[f32]) -> io::Result<()> {
        let n = usize::from(self.channels);
        debug_assert_eq!(interleaved.len() % n, 0, "whole frames only");
        self.bytes.clear();
        self.bytes.reserve(interleaved.len() * 4);
        for v in interleaved {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.out.write_all(&self.bytes)?;
        self.frames += (interleaved.len() / n) as u64;
        Ok(())
    }

    /// Writes the final sizes into the header and syncs the file to disk.
    pub fn finish(self) -> io::Result<WavInfo> {
        let (channels, sample_rate, frames, limit) = (
            self.channels,
            self.sample_rate,
            self.frames,
            self.rf64_above,
        );
        let mut f = self.out.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header(channels, sample_rate, frames, limit))?;
        f.sync_all()?;
        Ok(WavInfo {
            sample_rate,
            channels,
            frames,
        })
    }
}

/// Size of a finished file of `frames` frames of `channels` channels.
pub fn file_bytes(channels: u16, frames: u64) -> u64 {
    HEADER_BYTES + frames * u64::from(channels) * SAMPLE_BYTES
}

/// Finishes a file this module began but never finished (the writer was killed): the
/// frames are what reached the disk, a trailing partial frame is cut off.
pub fn repair(path: &Path) -> Result<WavInfo, WavError> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let mut h = [0u8; 116];
    f.read_exact(&mut h).map_err(|_| WavError::NotWav)?;
    if !(&h[0..4] == b"RIFF" || &h[0..4] == b"RF64")
        || &h[8..12] != b"WAVE"
        || &h[48..52] != b"fmt "
        || &h[108..112] != b"data"
    {
        return Err(WavError::Format(
            "not a raw capture file written by ac2".into(),
        ));
    }
    let channels = u16::from_le_bytes([h[58], h[59]]);
    let sample_rate = u32::from_le_bytes([h[60], h[61], h[62], h[63]]);
    if channels == 0 {
        return Err(WavError::Malformed("zero channels".into()));
    }
    let len = f.metadata()?.len();
    let block = u64::from(channels) * SAMPLE_BYTES;
    let frames = len.saturating_sub(HEADER_BYTES) / block;
    f.set_len(file_bytes(channels, frames))?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&header(channels, sample_rate, frames, RIFF_LIMIT))?;
    f.sync_all()?;
    Ok(WavInfo {
        sample_rate,
        channels,
        frames,
    })
}

/// How a file's samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    F32,
    /// Integer PCM of this many bytes (2, 3 or 4), full scale ±1.0 as in float files.
    Pcm(u8),
}

impl Encoding {
    fn bytes(self) -> usize {
        match self {
            Encoding::F32 => 4,
            Encoding::Pcm(n) => usize::from(n),
        }
    }

    fn decode(self, b: &[u8]) -> f32 {
        match self {
            Encoding::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Encoding::Pcm(2) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0,
            Encoding::Pcm(3) => {
                (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
            }
            Encoding::Pcm(_) => {
                (f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])) / 2_147_483_648.0) as f32
            }
        }
    }
}

/// Reads interleaved `f32` frames of a float WAV / RF64 file (what ac2 records), or of an
/// integer PCM file of 16, 24 or 32 bits (what a recorder writes) scaled to the same full
/// scale, so a recorder's file can be imported ([`super::import_wav`]).
#[derive(Debug)]
pub struct WavReader {
    input: BufReader<File>,
    info: WavInfo,
    encoding: Encoding,
    /// Frames read so far.
    pos: u64,
    bytes: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

impl WavReader {
    /// Opens `path` and reads its header.
    pub fn open(path: &Path) -> Result<Self, WavError> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        let mut input = BufReader::with_capacity(WRITE_BUFFER, f);
        let mut riff = [0u8; 12];
        input.read_exact(&mut riff).map_err(|_| WavError::NotWav)?;
        let rf64 = match &riff[0..4] {
            b"RIFF" => false,
            b"RF64" => true,
            _ => return Err(WavError::NotWav),
        };
        if &riff[8..12] != b"WAVE" {
            return Err(WavError::NotWav);
        }
        let mut at = 12u64;
        let mut ds64_data: Option<u64> = None;
        let mut fmt: Option<(u16, u32, Encoding)> = None;
        loop {
            let mut ch = [0u8; 8];
            input
                .read_exact(&mut ch)
                .map_err(|_| WavError::Malformed("no data chunk".into()))?;
            let size = u64::from(u32_at(&ch, 4));
            at += 8;
            match &ch[0..4] {
                b"ds64" => {
                    let mut b = vec![0u8; usize::try_from(size).unwrap_or(0).max(24)];
                    input
                        .read_exact(&mut b[..usize::try_from(size).unwrap_or(0)])
                        .map_err(|_| WavError::Malformed("short ds64 chunk".into()))?;
                    ds64_data = Some(u64_at(&b, 8));
                }
                b"fmt " => {
                    if !(16..=1024).contains(&size) {
                        return Err(WavError::Malformed(format!("fmt chunk of {size} bytes")));
                    }
                    let mut b = vec![0u8; usize::try_from(size).unwrap_or(0)];
                    input
                        .read_exact(&mut b)
                        .map_err(|_| WavError::Malformed("short fmt chunk".into()))?;
                    let tag = u16_at(&b, 0);
                    let channels = u16_at(&b, 2);
                    let rate = u32_at(&b, 4);
                    let bits = u16_at(&b, 14);
                    // The extensible subformat GUIDs differ from each other only in their
                    // first two bytes, which carry the plain format tag.
                    let kind = match tag {
                        FORMAT_EXTENSIBLE if b.len() >= 40 && b[26..40] == FLOAT_GUID[2..] => {
                            u16_at(&b, 24)
                        }
                        t => t,
                    };
                    let encoding = match (kind, bits) {
                        (FORMAT_FLOAT, 32) => Encoding::F32,
                        (FORMAT_PCM, 16 | 24 | 32) => Encoding::Pcm((bits / 8) as u8),
                        _ => {
                            return Err(WavError::Format(format!(
                                "format tag {tag:#06x}, {bits} bits; this reads 32-bit float \
                                 and 16-, 24- or 32-bit integer PCM"
                            )));
                        }
                    };
                    if channels == 0 || rate == 0 {
                        return Err(WavError::Malformed("zero channels or rate".into()));
                    }
                    fmt = Some((channels, rate, encoding));
                }
                b"data" => {
                    let (channels, sample_rate, encoding) =
                        fmt.ok_or_else(|| WavError::Malformed("data before fmt".into()))?;
                    let available = len.saturating_sub(at);
                    let stated = if rf64 && size == RIFF_LIMIT {
                        ds64_data.ok_or_else(|| {
                            WavError::Malformed("RF64 without a ds64 chunk".into())
                        })?
                    } else {
                        size
                    };
                    let block = u64::from(channels) * encoding.bytes() as u64;
                    if stated > available {
                        return Err(WavError::Malformed(format!(
                            "data chunk states {stated} bytes, the file holds {available}"
                        )));
                    }
                    return Ok(Self {
                        input,
                        info: WavInfo {
                            sample_rate,
                            channels,
                            frames: stated / block,
                        },
                        encoding,
                        pos: 0,
                        bytes: Vec::new(),
                    });
                }
                _ => {
                    // Chunks are word aligned.
                    let skip = size + (size & 1);
                    input.seek_relative(i64::try_from(skip).unwrap_or(i64::MAX))?;
                    at += skip;
                    continue;
                }
            }
            at += size;
        }
    }

    /// The file's format and length.
    pub fn info(&self) -> WavInfo {
        self.info
    }

    /// Reads up to `out.len() / channels` frames into `out`; returns the frames read, 0 at
    /// the end.
    pub fn read(&mut self, out: &mut [f32]) -> io::Result<usize> {
        let n = usize::from(self.info.channels);
        let left = self.info.frames - self.pos;
        let frames = (out.len() / n).min(usize::try_from(left).unwrap_or(usize::MAX));
        if frames == 0 {
            return Ok(0);
        }
        let size = self.encoding.bytes();
        self.bytes.resize(frames * n * size, 0);
        self.input.read_exact(&mut self.bytes)?;
        for (v, b) in out.iter_mut().zip(self.bytes.chunks_exact(size)) {
            *v = self.encoding.decode(b);
        }
        self.pos += frames as u64;
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize, ch: usize) -> Vec<f32> {
        (0..n * ch)
            .map(|i| {
                f32::from_bits(0x3e00_0000 + (i as u32).wrapping_mul(2_654_435_761) % 0x0100_0000)
            })
            .collect()
    }

    #[test]
    fn samples_come_back_bit_exact() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path().join("a.wav");
        let data = frames(1000, 3);
        let mut w = WavWriter::create(&p, 3, 48_000).expect("create");
        w.write(&data[..300]).expect("write");
        w.write(&data[300..]).expect("write");
        let info = w.finish().expect("finish");
        assert_eq!(info.frames, 1000);
        assert_eq!(
            std::fs::metadata(&p).expect("len").len(),
            file_bytes(3, 1000)
        );
        let mut r = WavReader::open(&p).expect("open");
        assert_eq!(r.info(), info);
        let mut back = vec![0.0f32; 3000 + 30];
        let mut got = 0;
        loop {
            let n = r
                .read(&mut back[got * 3..(got * 3 + 512).min(3030)])
                .expect("read");
            if n == 0 {
                break;
            }
            got += n;
        }
        assert_eq!(got, 1000);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&back[..3000]), bits(&data));
    }

    #[test]
    fn an_existing_file_is_never_replaced() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path().join("a.wav");
        std::fs::write(&p, b"keep").expect("write");
        assert!(WavWriter::create(&p, 1, 48_000).is_err());
        assert_eq!(std::fs::read(&p).expect("read"), b"keep");
    }

    #[test]
    fn past_the_riff_limit_the_header_becomes_rf64_without_moving_audio() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path().join("big.wav");
        let data = frames(64, 2);
        // A limit of 200 bytes stands in for 4 GiB.
        let mut w = WavWriter::create_with_limit(&p, 2, 96_000, 200).expect("create");
        w.write(&data).expect("write");
        w.finish().expect("finish");
        let raw = std::fs::read(&p).expect("read");
        assert_eq!(&raw[0..4], b"RF64");
        assert_eq!(&raw[12..16], b"ds64");
        assert_eq!(u32_at(&raw, 112), u32::MAX);
        assert_eq!(u64_at(&raw, 28), 64 * 8);
        let mut r = WavReader::open(&p).expect("open");
        assert_eq!(r.info().frames, 64);
        let mut back = vec![0.0f32; 128];
        assert_eq!(r.read(&mut back).expect("read"), 64);
        assert_eq!(back, data);
    }

    #[test]
    fn an_unfinished_file_is_repaired_from_what_reached_the_disk() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path().join("cut.wav");
        let data = frames(100, 2);
        let mut w = WavWriter::create(&p, 2, 48_000).expect("create");
        w.write(&data).expect("write");
        // The process dies: the buffer reaches the disk, the header is never patched, and
        // half a frame trails.
        let mut f = w.out.into_inner().expect("flush");
        f.write_all(&[1, 2, 3, 4]).expect("write");
        drop(f);
        assert!(
            WavReader::open(&p).is_ok_and(|r| r.info().frames == 0),
            "an unfinished header states no audio"
        );
        let info = repair(&p).expect("repair");
        assert_eq!(info.frames, 100);
        let mut r = WavReader::open(&p).expect("open");
        let mut back = vec![0.0f32; 200];
        assert_eq!(r.read(&mut back).expect("read"), 100);
        assert_eq!(back, data);
    }

    #[test]
    fn plain_float_and_integer_pcm_wavs_read() {
        let dir = tempfile::tempdir().expect("dir");
        let mut wav = Vec::new();
        let fmt = |tag: u16, bits: u16| {
            let mut b = Vec::new();
            b.extend_from_slice(b"fmt ");
            b.extend_from_slice(&16u32.to_le_bytes());
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&44_100u32.to_le_bytes());
            b.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes());
            b.extend_from_slice(&bits.to_le_bytes());
            b
        };
        let build = |tag, bits, wav: &mut Vec<u8>| {
            wav.clear();
            let f = fmt(tag, bits);
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(4 + f.len() as u32 + 8 + 8).to_le_bytes());
            wav.extend_from_slice(b"WAVE");
            wav.extend_from_slice(b"LIST");
            wav.extend_from_slice(&3u32.to_le_bytes());
            wav.extend_from_slice(&[0, 0, 0, 0]);
            wav.extend_from_slice(&f);
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&8u32.to_le_bytes());
            wav.extend_from_slice(&0.5f32.to_le_bytes());
            wav.extend_from_slice(&(-0.25f32).to_le_bytes());
        };
        let p = dir.path().join("plain.wav");
        build(FORMAT_FLOAT, 32, &mut wav);
        std::fs::write(&p, &wav).expect("write");
        let mut r = WavReader::open(&p).expect("open");
        assert_eq!(r.info().frames, 2);
        let mut b = [0.0f32; 2];
        r.read(&mut b).expect("read");
        assert_eq!(b, [0.5, -0.25]);
        // The same 8 data bytes as 16-bit PCM: four samples.
        build(FORMAT_PCM, 16, &mut wav);
        std::fs::write(&p, &wav).expect("write");
        let mut r = WavReader::open(&p).expect("open");
        assert_eq!(r.info().frames, 4);
        let mut b = [0.0f32; 4];
        r.read(&mut b).expect("read");
        let words: Vec<f32> = wav[wav.len() - 8..]
            .chunks_exact(2)
            .map(|w| f32::from(i16::from_le_bytes([w[0], w[1]])) / 32_768.0)
            .collect();
        assert_eq!(b.to_vec(), words);
        // 24-bit: -1/2 full scale and the smallest step.
        let mut pcm24 = Vec::new();
        for v in [-4_194_304i32, 1] {
            pcm24.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        build(FORMAT_PCM, 24, &mut wav);
        let at = wav.len() - 8;
        wav.truncate(at - 4);
        wav.extend_from_slice(&6u32.to_le_bytes());
        wav.extend_from_slice(&pcm24);
        std::fs::write(&p, &wav).expect("write");
        let mut r = WavReader::open(&p).expect("open");
        assert_eq!(r.info().frames, 2);
        let mut b = [0.0f32; 2];
        r.read(&mut b).expect("read");
        assert_eq!(b, [-0.5, 1.0 / 8_388_608.0]);
        build(FORMAT_PCM, 8, &mut wav);
        std::fs::write(&p, &wav).expect("write");
        assert!(matches!(WavReader::open(&p), Err(WavError::Format(_))));
    }
}
