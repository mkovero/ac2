//! ASIO sample types: which ones convert, and the conversion of one sample.
//!
//! ASIO buffers are per channel (mono), and each channel states its own sample type. Only
//! little-endian types are taken: the big-endian ones exist only on old Mac drivers.

use crate::backend::SampleFormat;

/// A channel buffer layout this backend converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    /// `ASIOSTInt16LSB`.
    I16,
    /// `ASIOSTInt24LSB`: packed, three bytes per sample.
    I24,
    /// `ASIOSTInt32LSB`.
    I32,
    /// `ASIOSTFloat32LSB`.
    F32,
    /// `ASIOSTFloat64LSB`.
    F64,
    /// `ASIOSTInt32LSB16/18/20/24`: a `bits`-bit sample right-justified in a 32-bit word.
    I32Justified {
        /// Significant bits (16, 18, 20 or 24).
        bits: u32,
    },
}

impl Format {
    /// The format of ASIO sample type `t`, if it converts.
    pub(crate) fn from_asio(t: i32) -> Option<Self> {
        Some(match t {
            16 => Self::I16,
            17 => Self::I24,
            18 => Self::I32,
            19 => Self::F32,
            20 => Self::F64,
            24 => Self::I32Justified { bits: 16 },
            25 => Self::I32Justified { bits: 18 },
            26 => Self::I32Justified { bits: 20 },
            27 => Self::I32Justified { bits: 24 },
            _ => return None,
        })
    }

    /// The ASIO name of sample type `t`, for an error naming a type that does not convert.
    pub(crate) fn asio_name(t: i32) -> String {
        let name = match t {
            0 => "Int16MSB",
            1 => "Int24MSB",
            2 => "Int32MSB",
            3 => "Float32MSB",
            4 => "Float64MSB",
            8..=11 => "Int32MSB (justified)",
            16 => "Int16LSB",
            17 => "Int24LSB",
            18 => "Int32LSB",
            19 => "Float32LSB",
            20 => "Float64LSB",
            24..=27 => "Int32LSB (justified)",
            32 | 33 | 40 => "DSD",
            _ => return format!("sample type {t}"),
        };
        name.into()
    }

    /// Bytes per sample in the driver's buffer.
    pub(crate) fn bytes(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24 => 3,
            Self::I32 | Self::F32 | Self::I32Justified { .. } => 4,
            Self::F64 => 8,
        }
    }

    /// The format as the crate names it.
    pub(crate) fn sample_format(self) -> SampleFormat {
        match self {
            Self::I16 => SampleFormat::I16,
            Self::I24 => SampleFormat::I24,
            Self::I32 | Self::I32Justified { .. } => SampleFormat::I32,
            Self::F32 => SampleFormat::F32,
            Self::F64 => SampleFormat::F64,
        }
    }

    /// Full scale of an integer format: the value that reads as 1.0.
    fn scale(self) -> f64 {
        match self {
            Self::I16 => 32_768.0,
            Self::I24 => 8_388_608.0,
            Self::I32 => 2_147_483_648.0,
            Self::I32Justified { bits } => f64::from(1u32 << (bits - 1)),
            Self::F32 | Self::F64 => 1.0,
        }
    }

    /// Sample `frame` of `buf`.
    ///
    /// # Safety
    /// `buf` holds at least `frame + 1` samples of this format.
    #[inline]
    pub(crate) unsafe fn read(self, buf: *const u8, frame: usize) -> f32 {
        // SAFETY: the caller guarantees the sample is inside the buffer; drivers do not
        // promise alignment of a packed 24-bit buffer, so every read is unaligned.
        let p = unsafe { buf.add(frame * self.bytes()) };
        let le = |n: usize| -> [u8; 8] {
            let mut b = [0u8; 8];
            // SAFETY: as above, `n` bytes at `p` are inside the buffer.
            unsafe { std::ptr::copy_nonoverlapping(p, b.as_mut_ptr(), n) };
            b
        };
        match self {
            Self::F32 => {
                let b = le(4);
                f32::from_le_bytes([b[0], b[1], b[2], b[3]])
            }
            Self::F64 => f64::from_le_bytes(le(8)) as f32,
            Self::I16 => {
                let b = le(2);
                (f64::from(i16::from_le_bytes([b[0], b[1]])) / self.scale()) as f32
            }
            Self::I24 => {
                let b = le(3);
                // Into the top three bytes, then an arithmetic shift sign-extends.
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                (f64::from(v) / self.scale()) as f32
            }
            Self::I32 => {
                let b = le(4);
                (f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])) / self.scale()) as f32
            }
            Self::I32Justified { bits } => {
                let b = le(4);
                let raw = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                // Sign-extend from the sample's own top bit: a driver may leave the unused
                // high bits zero rather than copies of the sign.
                let v = (raw << (32 - bits)) >> (32 - bits);
                (f64::from(v) / self.scale()) as f32
            }
        }
    }

    /// Writes `v` (clamped to ±1, rounded to the nearest code) as sample `frame` of `buf`.
    ///
    /// # Safety
    /// `buf` holds at least `frame + 1` samples of this format.
    #[inline]
    pub(crate) unsafe fn write(self, buf: *mut u8, frame: usize, v: f32) {
        let v = f64::from(if v.is_nan() { 0.0 } else { v.clamp(-1.0, 1.0) });
        let int = |s: f64| (v * s).round().clamp(-s, s - 1.0) as i64;
        let mut b = [0u8; 8];
        let n = self.bytes();
        match self {
            Self::F32 => b[..4].copy_from_slice(&(v as f32).to_le_bytes()),
            Self::F64 => b.copy_from_slice(&v.to_le_bytes()),
            Self::I16 | Self::I24 | Self::I32 | Self::I32Justified { .. } => {
                // Two's complement little-endian: the low `n` bytes of the i64 are the
                // sample, sign bits included (a justified sample is sign-extended to 32 bits).
                b = int(self.scale()).to_le_bytes();
            }
        }
        // SAFETY: the caller guarantees the sample is inside the buffer; unaligned as in
        // `read`.
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), buf.add(frame * n), n) };
    }

    /// Fills `frames` samples of `buf` with silence. Zero is all-zero bits in every format.
    ///
    /// # Safety
    /// `buf` holds at least `frames` samples of this format.
    #[inline]
    pub(crate) unsafe fn silence(self, buf: *mut u8, frames: usize) {
        // SAFETY: as stated by the caller.
        unsafe { std::ptr::write_bytes(buf, 0, frames * self.bytes()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Format; 9] = [
        Format::I16,
        Format::I24,
        Format::I32,
        Format::F32,
        Format::F64,
        Format::I32Justified { bits: 16 },
        Format::I32Justified { bits: 18 },
        Format::I32Justified { bits: 20 },
        Format::I32Justified { bits: 24 },
    ];

    fn write(f: Format, v: f32) -> Vec<u8> {
        let mut b = vec![0xAAu8; f.bytes()];
        // SAFETY: one sample of `f`.
        unsafe { f.write(b.as_mut_ptr(), 0, v) };
        b
    }

    fn read(f: Format, b: &[u8]) -> f32 {
        // SAFETY: `b` holds one sample of `f`.
        unsafe { f.read(b.as_ptr(), 0) }
    }

    #[test]
    fn every_asio_type_number_maps_as_the_spec_lists_it() {
        let expect = [
            (16, Some(Format::I16)),
            (17, Some(Format::I24)),
            (18, Some(Format::I32)),
            (19, Some(Format::F32)),
            (20, Some(Format::F64)),
            (24, Some(Format::I32Justified { bits: 16 })),
            (27, Some(Format::I32Justified { bits: 24 })),
            (0, None),
            (3, None),
            (11, None),
            (32, None),
        ];
        for (t, f) in expect {
            assert_eq!(Format::from_asio(t), f, "type {t}");
        }
        assert_eq!(Format::asio_name(3), "Float32MSB");
    }

    #[test]
    fn integer_codes_are_exact_at_full_scale_half_and_one_lsb() {
        // Analytic bit patterns: little-endian two's complement.
        assert_eq!(write(Format::I16, 0.5), 16_384i16.to_le_bytes());
        assert_eq!(write(Format::I16, -1.0), (-32_768i16).to_le_bytes());
        assert_eq!(write(Format::I16, 1.0), 32_767i16.to_le_bytes());
        assert_eq!(write(Format::I24, 0.5), [0x00, 0x00, 0x40]);
        assert_eq!(write(Format::I24, -1.0), [0x00, 0x00, 0x80]);
        assert_eq!(write(Format::I24, -1.0 / 8_388_608.0), [0xFF, 0xFF, 0xFF]);
        assert_eq!(write(Format::I32, -1.0), i32::MIN.to_le_bytes());
        assert_eq!(write(Format::I32, 1.0), i32::MAX.to_le_bytes());
        assert_eq!(
            write(Format::I32Justified { bits: 24 }, -0.5),
            (-4_194_304i32).to_le_bytes()
        );
        assert_eq!(
            write(Format::I32Justified { bits: 16 }, 0.25),
            8_192i32.to_le_bytes()
        );
        assert_eq!(
            read(Format::I24, &[0xFF, 0xFF, 0x7F]),
            8_388_607.0 / 8_388_608.0
        );
        assert_eq!(read(Format::I24, &[0x00, 0x00, 0x80]), -1.0);
        assert_eq!(read(Format::I16, &(-16_384i16).to_le_bytes()), -0.5);
        assert_eq!(read(Format::I32, &i32::MIN.to_le_bytes()), -1.0);
    }

    #[test]
    fn justified_samples_sign_extend_from_their_own_top_bit() {
        // An 18-bit -1 LSB with the unused high bits left zero.
        let raw = 0x0003_FFFFi32.to_le_bytes();
        assert_eq!(
            read(Format::I32Justified { bits: 18 }, &raw),
            -1.0 / 131_072.0
        );
        let full = (-1i32).to_le_bytes();
        assert_eq!(
            read(Format::I32Justified { bits: 18 }, &full),
            -1.0 / 131_072.0
        );
    }

    #[test]
    fn round_trips_within_half_an_lsb_and_clamps_out_of_range() {
        for f in ALL {
            let lsb = 1.0 / f.scale();
            for v in [0.0f32, 0.1, -0.1, 0.333, -0.999, 0.75] {
                let back = read(f, &write(f, v));
                let tol = (lsb / 2.0).max(f64::from(f32::EPSILON));
                assert!(
                    (f64::from(back) - f64::from(v)).abs() <= tol,
                    "{f:?} {v} -> {back}"
                );
            }
            assert!(read(f, &write(f, 3.0)) <= 1.0, "{f:?}");
            assert!(read(f, &write(f, -3.0)) >= -1.0, "{f:?}");
            assert_eq!(read(f, &write(f, f32::NAN)), 0.0, "{f:?}");
        }
    }

    #[test]
    fn silence_is_zero_in_every_format_and_reads_unaligned() {
        for f in ALL {
            let mut b = vec![0x55u8; 1 + 4 * f.bytes()];
            // SAFETY: four samples after the one-byte offset.
            unsafe { f.silence(b.as_mut_ptr().add(1), 4) };
            assert_eq!(b[0], 0x55);
            for i in 0..4 {
                // SAFETY: as above; the offset makes every sample unaligned.
                assert_eq!(unsafe { f.read(b.as_ptr().add(1), i) }, 0.0, "{f:?}");
            }
        }
    }
}
