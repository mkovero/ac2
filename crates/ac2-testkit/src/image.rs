//! Golden images: RGBA8 PNG I/O, per-pixel tolerance comparison, bless/compare helper.
//!
//! Rasterizers (lavapipe, llvmpipe, WARP, hardware) agree on interior pixels and differ by a
//! few LSB on anti-aliased edges, where coverage comes from f32 shader math and blending
//! rounds differently. A pixel therefore passes when every channel is within
//! [`ImageTolerance::channel`], and an image passes when at most
//! [`ImageTolerance::max_bad_fraction`] of its pixels fail. A real regression (a stroke one
//! pixel off, a missing segment, a double-blended join) moves a run of pixels by far more.

use std::fmt;
use std::path::{Path, PathBuf};

/// Set to `1` to rewrite reference images instead of comparing against them.
pub const BLESS_ENV: &str = "AC2_BLESS";
/// Set to `1` to make a missing GPU adapter a test failure instead of a skip (CI).
pub const REQUIRE_GPU_ENV: &str = "AC2_REQUIRE_GPU";

fn env_is_one(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// `AC2_BLESS=1` is set.
pub fn bless_requested() -> bool {
    env_is_one(BLESS_ENV)
}

/// `AC2_REQUIRE_GPU=1` is set: a test that cannot get an adapter must fail, not skip.
pub fn gpu_required() -> bool {
    env_is_one(REQUIRE_GPU_ENV)
}

/// Where failing comparisons write actual/expected/diff images: `<target>/golden-images/`.
/// Honours `CARGO_TARGET_DIR`; CI uploads this directory on failure.
pub fn artifacts_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("golden-images")
}

/// Host-side RGBA8 image, rows top to bottom, no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

#[derive(Debug)]
pub enum ImageError {
    /// `rgba.len()` is not `width * height * 4`.
    Size {
        width: u32,
        height: u32,
        len: usize,
    },
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Decode {
        path: PathBuf,
        error: String,
    },
    Encode {
        path: PathBuf,
        error: String,
    },
    /// The PNG is not 8-bit RGBA.
    Format {
        path: PathBuf,
        color: String,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Size { width, height, len } => {
                write!(
                    f,
                    "{width}x{height} RGBA8 needs {} bytes, got {len}",
                    width * height * 4
                )
            }
            ImageError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            ImageError::Decode { path, error } => write!(f, "{}: decode: {error}", path.display()),
            ImageError::Encode { path, error } => write!(f, "{}: encode: {error}", path.display()),
            ImageError::Format { path, color } => {
                write!(f, "{}: expected 8-bit RGBA, got {color}", path.display())
            }
        }
    }
}

impl std::error::Error for ImageError {}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, ImageError> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(ImageError::Size {
                width,
                height,
                len: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Pixel at `(x, y)`; panics when out of bounds (test helper).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x},{y}) outside image"
        );
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    pub fn load_png(path: &Path) -> Result<Self, ImageError> {
        let file = std::fs::File::open(path).map_err(|error| ImageError::Io {
            path: path.to_owned(),
            error,
        })?;
        let decode = |error: png::DecodingError| ImageError::Decode {
            path: path.to_owned(),
            error: error.to_string(),
        };
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .map_err(decode)?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| ImageError::Decode {
                path: path.to_owned(),
                error: "image too large".into(),
            })?;
        let mut buf = vec![0; size];
        let info = reader.next_frame(&mut buf).map_err(decode)?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return Err(ImageError::Format {
                path: path.to_owned(),
                color: format!("{:?}/{:?}", info.color_type, info.bit_depth),
            });
        }
        buf.truncate(info.buffer_size());
        Self::new(info.width, info.height, buf)
    }

    /// Writes an RGBA8 PNG, creating parent directories.
    pub fn save_png(&self, path: &Path) -> Result<(), ImageError> {
        let io = |error| ImageError::Io {
            path: path.to_owned(),
            error,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let file = std::fs::File::create(path).map_err(io)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let encode = |error: png::EncodingError| ImageError::Encode {
            path: path.to_owned(),
            error: error.to_string(),
        };
        let mut w = enc.write_header().map_err(encode)?;
        w.write_image_data(&self.rgba).map_err(encode)?;
        w.finish().map_err(encode)
    }
}

/// See the module docs for why the tolerance has two parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageTolerance {
    /// Largest per-channel |difference| for a pixel to count as matching.
    pub channel: u8,
    /// Largest fraction of non-matching pixels for the image to pass.
    pub max_bad_fraction: f64,
}

/// Result of [`compare`].
#[derive(Clone, Debug)]
pub struct ImageDiff {
    /// Pixels with some channel differing by more than the tolerance.
    pub bad_pixels: usize,
    /// Pixels differing at all.
    pub nonzero_pixels: usize,
    pub total_pixels: usize,
    pub max_channel_diff: u8,
    /// Visualisation: red where outside tolerance, green tint scaled by the difference
    /// where within, dimmed expected image elsewhere.
    pub image: Image,
}

impl ImageDiff {
    pub fn bad_fraction(&self) -> f64 {
        if self.total_pixels == 0 {
            0.0
        } else {
            self.bad_pixels as f64 / self.total_pixels as f64
        }
    }

    pub fn passes(&self, tol: ImageTolerance) -> bool {
        self.bad_fraction() <= tol.max_bad_fraction
    }

    /// One-line summary for logs.
    pub fn summary(&self, tol: ImageTolerance) -> String {
        format!(
            "{} px differ at all, {} px > {} ({:.4} % of {}, limit {:.4} %), max channel diff {}",
            self.nonzero_pixels,
            self.bad_pixels,
            tol.channel,
            self.bad_fraction() * 100.0,
            self.total_pixels,
            tol.max_bad_fraction * 100.0,
            self.max_channel_diff
        )
    }
}

/// Compares two images of equal size; `None` when sizes differ.
pub fn compare(actual: &Image, expected: &Image, tol: ImageTolerance) -> Option<ImageDiff> {
    if (actual.width, actual.height) != (expected.width, expected.height) {
        return None;
    }
    let mut bad = 0;
    let mut nonzero = 0;
    let mut max = 0u8;
    let mut vis = Vec::with_capacity(actual.rgba.len());
    for (a, e) in actual
        .rgba
        .chunks_exact(4)
        .zip(expected.rgba.chunks_exact(4))
    {
        let d = a
            .iter()
            .zip(e)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        max = max.max(d);
        nonzero += usize::from(d > 0);
        if d > tol.channel {
            bad += 1;
            vis.extend_from_slice(&[255, 0, 0, 255]);
        } else {
            let g = e[0] / 4;
            vis.extend_from_slice(&[g, g.saturating_add(d.saturating_mul(8)), g, 255]);
        }
    }
    Some(ImageDiff {
        bad_pixels: bad,
        nonzero_pixels: nonzero,
        total_pixels: actual.width as usize * actual.height as usize,
        max_channel_diff: max,
        image: Image {
            width: actual.width,
            height: actual.height,
            rgba: vis,
        },
    })
}

/// Outcome of [`check_golden`] when it does not fail.
#[derive(Clone, Debug)]
pub enum GoldenOutcome {
    /// `AC2_BLESS=1`: the reference was (re)written.
    Blessed(PathBuf),
    /// Within tolerance.
    Matched(ImageDiff),
}

/// Compares `actual` with `<reference_dir>/<name>.png`, or rewrites the reference under
/// `AC2_BLESS=1`. On mismatch, writes `<name>_actual.png`, `<name>_expected.png` and
/// `<name>_diff.png` to [`artifacts_dir`] and returns a message naming them.
pub fn check_golden(
    name: &str,
    actual: &Image,
    reference_dir: &Path,
    tol: ImageTolerance,
) -> Result<GoldenOutcome, String> {
    let reference = reference_dir.join(format!("{name}.png"));
    if bless_requested() {
        actual.save_png(&reference).map_err(|e| e.to_string())?;
        return Ok(GoldenOutcome::Blessed(reference));
    }
    let expected = Image::load_png(&reference)
        .map_err(|e| format!("{name}: {e} (create it with {BLESS_ENV}=1)"))?;
    let dir = artifacts_dir();
    let save = |suffix: &str, img: &Image| {
        let p = dir.join(format!("{name}_{suffix}.png"));
        img.save_png(&p).map(|()| p).map_err(|e| e.to_string())
    };
    let Some(diff) = compare(actual, &expected, tol) else {
        let p = save("actual", actual)?;
        return Err(format!(
            "{name}: size {}x{} != reference {}x{}; actual written to {}",
            actual.width,
            actual.height,
            expected.width,
            expected.height,
            p.display()
        ));
    };
    if diff.passes(tol) {
        return Ok(GoldenOutcome::Matched(diff));
    }
    save("actual", actual)?;
    save("expected", &expected)?;
    save("diff", &diff.image)?;
    Err(format!(
        "{name}: differs from {} beyond tolerance: {}; images in {}",
        reference.display(),
        diff.summary(tol),
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_counts_out_of_tolerance_pixels() {
        let a = Image::new(2, 1, vec![10, 10, 10, 255, 0, 0, 0, 255]).expect("size");
        let mut b = a.clone();
        b.rgba[0] = 13;
        b.rgba[4] = 100;
        let tol = ImageTolerance {
            channel: 4,
            max_bad_fraction: 0.4,
        };
        let d = compare(&a, &b, tol).expect("same size");
        assert_eq!(d.bad_pixels, 1);
        assert_eq!(d.nonzero_pixels, 2);
        assert_eq!(d.max_channel_diff, 100);
        assert!(!d.passes(tol));
        assert!(d.passes(ImageTolerance {
            channel: 4,
            max_bad_fraction: 0.5
        }));
    }

    #[test]
    fn size_mismatch_is_not_comparable() {
        let a = Image::new(1, 1, vec![0; 4]).expect("size");
        let b = Image::new(1, 2, vec![0; 8]).expect("size");
        let tol = ImageTolerance {
            channel: 0,
            max_bad_fraction: 0.0,
        };
        assert!(compare(&a, &b, tol).is_none());
        assert!(Image::new(2, 2, vec![0; 4]).is_err());
    }

    #[test]
    fn png_round_trip() {
        let dir = artifacts_dir().join("testkit-selftest");
        let p = dir.join("rt.png");
        let img = Image::new(3, 2, (0..24).map(|i| (i * 10) as u8).collect()).expect("size");
        img.save_png(&p).expect("save");
        assert_eq!(Image::load_png(&p).expect("load"), img);
    }
}
