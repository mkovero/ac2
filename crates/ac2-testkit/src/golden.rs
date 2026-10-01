//! Loader for the `ac2-golden` vector format written by `tools/refgen/generate.py`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use num_complex::Complex64;
use serde::Deserialize;

use crate::compare::{Comparison, DbTolerance, Mismatch, Tolerance};

/// Format identifier in every metadata file.
pub const FORMAT: &str = "ac2-golden";
/// Format version this loader understands.
pub const FORMAT_VERSION: u32 = 1;

/// `fixtures/golden/` in the workspace.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden")
}

/// Element type of an array in the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dtype {
    /// Little-endian IEEE-754 binary64.
    F64,
    /// Complex binary64, stored as interleaved little-endian `re, im` pairs.
    C128,
}

impl Dtype {
    /// Bytes per element.
    pub fn size(self) -> usize {
        match self {
            Dtype::F64 => 8,
            Dtype::C128 => 16,
        }
    }
}

impl fmt::Display for Dtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dtype::F64 => "f64",
            Dtype::C128 => "c128",
        })
    }
}

/// Tolerance the generator suggests for an independent implementation.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SuggestedTolerance {
    /// `|actual − expected| ≤ abs + rel·|expected|`.
    Linear { abs: f64, rel: f64 },
    /// Values in dB, clamped below at `floor_db`, `|actual − expected| ≤ db`.
    Db { db: f64, floor_db: f64 },
}

/// Who produced a set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub script: String,
    pub function: String,
}

/// Blob file description.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobInfo {
    pub file: String,
    pub nbytes: usize,
    pub sha256: String,
}

/// One array in the blob.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrayMeta {
    pub name: String,
    pub dtype: Dtype,
    pub shape: Vec<usize>,
    pub offset: usize,
    pub nbytes: usize,
    pub unit: String,
    pub description: String,
    #[serde(default)]
    pub tolerance: Option<SuggestedTolerance>,
    /// Name of the array that holds this array's x-axis (frequency), if any.
    #[serde(default)]
    pub axis: Option<String>,
}

impl ArrayMeta {
    /// Number of elements (product of the shape).
    pub fn len(&self) -> usize {
        self.shape.iter().product()
    }

    /// True when the array has no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Contents of `<name>.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub format: String,
    pub format_version: u32,
    pub name: String,
    pub description: String,
    pub generator: Generator,
    pub versions: BTreeMap<String, String>,
    pub parameters: serde_json::Map<String, serde_json::Value>,
    pub references: Vec<String>,
    pub scalars: BTreeMap<String, f64>,
    pub blob: BlobInfo,
    pub arrays: Vec<ArrayMeta>,
}

/// Errors while loading or reading a golden set.
#[derive(Debug)]
pub enum GoldenError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// The files do not follow the format (version, sizes, offsets).
    Format {
        set: String,
        detail: String,
    },
    MissingArray {
        set: String,
        name: String,
    },
    MissingScalar {
        set: String,
        name: String,
    },
    MissingTolerance {
        set: String,
        name: String,
    },
    WrongDtype {
        set: String,
        name: String,
        expected: Dtype,
        found: Dtype,
    },
}

impl fmt::Display for GoldenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GoldenError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            GoldenError::Json { path, source } => write!(f, "{}: {source}", path.display()),
            GoldenError::Format { set, detail } => write!(f, "golden set {set}: {detail}"),
            GoldenError::MissingArray { set, name } => {
                write!(f, "golden set {set}: no array named {name}")
            }
            GoldenError::MissingScalar { set, name } => {
                write!(f, "golden set {set}: no scalar named {name}")
            }
            GoldenError::MissingTolerance { set, name } => {
                write!(
                    f,
                    "golden set {set}: array {name} has no suggested tolerance"
                )
            }
            GoldenError::WrongDtype {
                set,
                name,
                expected,
                found,
            } => write!(
                f,
                "golden set {set}: array {name} is {found}, requested as {expected}"
            ),
        }
    }
}

impl std::error::Error for GoldenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GoldenError::Io { source, .. } => Some(source),
            GoldenError::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A loaded vector set: metadata plus the raw blob.
#[derive(Debug, Clone)]
pub struct GoldenSet {
    meta: Metadata,
    blob: Vec<u8>,
}

impl GoldenSet {
    /// Load `<name>` from [`fixtures_dir`].
    pub fn load(name: &str) -> Result<Self, GoldenError> {
        Self::load_from(&fixtures_dir(), name)
    }

    /// Load `<dir>/<name>.json` and the blob it names.
    pub fn load_from(dir: &Path, name: &str) -> Result<Self, GoldenError> {
        let json_path = dir.join(format!("{name}.json"));
        let text = std::fs::read_to_string(&json_path).map_err(|source| GoldenError::Io {
            path: json_path.clone(),
            source,
        })?;
        let meta: Metadata = serde_json::from_str(&text).map_err(|source| GoldenError::Json {
            path: json_path.clone(),
            source,
        })?;
        let blob_path = dir.join(&meta.blob.file);
        let blob = std::fs::read(&blob_path).map_err(|source| GoldenError::Io {
            path: blob_path,
            source,
        })?;
        Self::from_parts(meta, blob)
    }

    /// Build from already-read metadata and blob, validating sizes and offsets.
    pub fn from_parts(meta: Metadata, blob: Vec<u8>) -> Result<Self, GoldenError> {
        let fail = |detail: String| GoldenError::Format {
            set: meta.name.clone(),
            detail,
        };
        if meta.format != FORMAT {
            return Err(fail(format!(
                "format is {:?}, expected {FORMAT:?}",
                meta.format
            )));
        }
        if meta.format_version != FORMAT_VERSION {
            return Err(fail(format!(
                "format_version {} not supported (expected {FORMAT_VERSION})",
                meta.format_version
            )));
        }
        if blob.len() != meta.blob.nbytes {
            return Err(fail(format!(
                "blob is {} bytes, metadata says {}",
                blob.len(),
                meta.blob.nbytes
            )));
        }
        for (i, a) in meta.arrays.iter().enumerate() {
            if meta.arrays[..i].iter().any(|b| b.name == a.name) {
                return Err(fail(format!("duplicate array {}", a.name)));
            }
            if a.len() * a.dtype.size() != a.nbytes {
                return Err(fail(format!(
                    "array {}: shape {:?} of {} needs {} bytes, nbytes is {}",
                    a.name,
                    a.shape,
                    a.dtype,
                    a.len() * a.dtype.size(),
                    a.nbytes
                )));
            }
            let end = a.offset.checked_add(a.nbytes);
            if end.is_none_or(|end| end > blob.len()) {
                return Err(fail(format!("array {} lies outside the blob", a.name)));
            }
            if let Some(axis) = &a.axis {
                let ok = meta
                    .arrays
                    .iter()
                    .any(|b| &b.name == axis && b.dtype == Dtype::F64 && b.len() == a.len());
                if !ok {
                    return Err(fail(format!(
                        "array {}: axis {axis} missing, not f64 or of different length",
                        a.name
                    )));
                }
            }
        }
        Ok(Self { meta, blob })
    }

    /// Set name.
    pub fn name(&self) -> &str {
        &self.meta.name
    }

    /// Full metadata.
    pub fn meta(&self) -> &Metadata {
        &self.meta
    }

    /// Descriptor of array `name`.
    pub fn array_meta(&self, name: &str) -> Result<&ArrayMeta, GoldenError> {
        self.meta
            .arrays
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| GoldenError::MissingArray {
                set: self.meta.name.clone(),
                name: name.to_owned(),
            })
    }

    fn bytes_of(&self, name: &str, dtype: Dtype) -> Result<&[u8], GoldenError> {
        let a = self.array_meta(name)?;
        if a.dtype != dtype {
            return Err(GoldenError::WrongDtype {
                set: self.meta.name.clone(),
                name: name.to_owned(),
                expected: dtype,
                found: a.dtype,
            });
        }
        // Range validated in from_parts.
        Ok(&self.blob[a.offset..a.offset + a.nbytes])
    }

    /// Real array `name`, flattened row-major.
    pub fn f64(&self, name: &str) -> Result<Vec<f64>, GoldenError> {
        Ok(self
            .bytes_of(name, Dtype::F64)?
            .chunks_exact(8)
            .map(le_f64)
            .collect())
    }

    /// Complex array `name`, flattened row-major.
    pub fn c128(&self, name: &str) -> Result<Vec<Complex64>, GoldenError> {
        Ok(self
            .bytes_of(name, Dtype::C128)?
            .chunks_exact(16)
            .map(|c| Complex64::new(le_f64(&c[..8]), le_f64(&c[8..])))
            .collect())
    }

    /// Named scalar result.
    pub fn scalar(&self, name: &str) -> Result<f64, GoldenError> {
        self.meta
            .scalars
            .get(name)
            .copied()
            .ok_or_else(|| GoldenError::MissingScalar {
                set: self.meta.name.clone(),
                name: name.to_owned(),
            })
    }

    /// Generator parameter (free-form JSON).
    pub fn parameter(&self, name: &str) -> Option<&serde_json::Value> {
        self.meta.parameters.get(name)
    }

    /// Suggested tolerance of array `name`.
    pub fn tolerance(&self, name: &str) -> Result<SuggestedTolerance, GoldenError> {
        self.array_meta(name)?
            .tolerance
            .ok_or_else(|| GoldenError::MissingTolerance {
                set: self.meta.name.clone(),
                name: name.to_owned(),
            })
    }

    /// The x-axis values of array `name`, if it declares one.
    pub fn axis_of(&self, name: &str) -> Result<Option<Vec<f64>>, GoldenError> {
        match &self.array_meta(name)?.axis {
            Some(axis) => self.f64(axis).map(Some),
            None => Ok(None),
        }
    }

    fn comparison_for(
        &self,
        name: &str,
    ) -> Result<(String, Option<Vec<f64>>, String), GoldenError> {
        let label = format!("{}/{}", self.meta.name, name);
        let axis = self.axis_of(name)?;
        let unit = match &self.array_meta(name)?.axis {
            Some(axis) => self.array_meta(axis)?.unit.clone(),
            None => String::new(),
        };
        Ok((label, axis, unit))
    }

    /// Compare `actual` with real array `name` using its suggested tolerance; failure
    /// messages carry the array's axis values when it has one.
    pub fn compare_f64(
        &self,
        name: &str,
        actual: &[f64],
    ) -> Result<Result<(), Box<Mismatch>>, GoldenError> {
        let expected = self.f64(name)?;
        let tol = self.tolerance(name)?;
        let (label, axis, unit) = self.comparison_for(name)?;
        let mut cmp = Comparison::new(&label);
        if let Some(axis) = &axis {
            cmp = cmp.with_axis(axis, &unit);
        }
        Ok(match tol {
            SuggestedTolerance::Linear { abs, rel } => {
                cmp.close_f64(&expected, actual, Tolerance { abs, rel })
            }
            SuggestedTolerance::Db { db, floor_db } => {
                cmp.close_db(&expected, actual, DbTolerance { db, floor_db })
            }
        })
    }

    /// Compare `actual` with complex array `name` using its suggested linear tolerance.
    pub fn compare_c128(
        &self,
        name: &str,
        actual: &[Complex64],
    ) -> Result<Result<(), Box<Mismatch>>, GoldenError> {
        let expected = self.c128(name)?;
        let tol = match self.tolerance(name)? {
            SuggestedTolerance::Linear { abs, rel } => Tolerance { abs, rel },
            SuggestedTolerance::Db { .. } => {
                return Err(GoldenError::Format {
                    set: self.meta.name.clone(),
                    detail: format!("complex array {name} has a dB tolerance"),
                });
            }
        };
        let (label, axis, unit) = self.comparison_for(name)?;
        let mut cmp = Comparison::new(&label);
        if let Some(axis) = &axis {
            cmp = cmp.with_axis(axis, &unit);
        }
        Ok(cmp.close_c64(&expected, actual, tol))
    }

    /// [`Self::compare_f64`], panicking with a readable report on any error or mismatch.
    #[track_caller]
    pub fn assert_f64(&self, name: &str, actual: &[f64]) {
        match self.compare_f64(name, actual) {
            Ok(r) => crate::compare::assert_ok(r),
            Err(e) => panic!("{e}"),
        }
    }

    /// [`Self::compare_c128`], panicking with a readable report on any error or mismatch.
    #[track_caller]
    pub fn assert_c128(&self, name: &str, actual: &[Complex64]) {
        match self.compare_c128(name, actual) {
            Ok(r) => crate::compare::assert_ok(r),
            Err(e) => panic!("{e}"),
        }
    }
}

fn le_f64(b: &[u8]) -> f64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    f64::from_le_bytes(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_json(arrays: &str, nbytes: usize) -> String {
        format!(
            r#"{{
  "format": "ac2-golden", "format_version": 1, "name": "t", "description": "",
  "generator": {{"script": "s", "function": "f"}},
  "versions": {{"numpy": "x"}}, "parameters": {{"n": 2}}, "references": [],
  "scalars": {{"lag": 3}},
  "blob": {{"file": "t.bin", "nbytes": {nbytes}, "sha256": ""}},
  "arrays": [{arrays}]
}}"#
        )
    }

    fn blob(values: &[f64]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn reads_f64_c128_and_scalars() {
        let arrays = r#"
          {"name": "f", "dtype": "f64", "shape": [2], "offset": 0, "nbytes": 16,
           "unit": "Hz", "description": ""},
          {"name": "h", "dtype": "c128", "shape": [2], "offset": 16, "nbytes": 32,
           "unit": "1", "description": "", "axis": "f",
           "tolerance": {"kind": "linear", "abs": 0.0, "rel": 1e-9}}"#;
        let meta: Metadata = serde_json::from_str(&meta_json(arrays, 48)).expect("json");
        let set = GoldenSet::from_parts(meta, blob(&[10.0, 20.0, 1.0, -2.0, 3.5, 0.25]))
            .expect("valid set");
        assert_eq!(set.f64("f").expect("f"), vec![10.0, 20.0]);
        assert_eq!(
            set.c128("h").expect("h"),
            vec![Complex64::new(1.0, -2.0), Complex64::new(3.5, 0.25)]
        );
        assert_eq!(set.scalar("lag").expect("lag"), 3.0);
        assert_eq!(set.axis_of("h").expect("axis"), Some(vec![10.0, 20.0]));
        assert!(matches!(
            set.f64("h"),
            Err(GoldenError::WrongDtype {
                found: Dtype::C128,
                ..
            })
        ));
        assert!(matches!(
            set.f64("nope"),
            Err(GoldenError::MissingArray { .. })
        ));
        assert!(matches!(
            set.tolerance("f"),
            Err(GoldenError::MissingTolerance { .. })
        ));
        let err = set
            .compare_c128("h", &[Complex64::new(1.0, -2.0), Complex64::new(3.5, 0.5)])
            .expect("comparable")
            .expect_err("must mismatch");
        let msg = err.to_string();
        assert!(msg.contains("t/h"), "{msg}");
        assert!(msg.contains("20 Hz"), "{msg}");
    }

    #[test]
    fn rejects_bad_sizes() {
        let arrays = r#"{"name": "f", "dtype": "f64", "shape": [3], "offset": 0, "nbytes": 16,
                         "unit": "", "description": ""}"#;
        let meta: Metadata = serde_json::from_str(&meta_json(arrays, 16)).expect("json");
        assert!(matches!(
            GoldenSet::from_parts(meta, blob(&[1.0, 2.0])),
            Err(GoldenError::Format { .. })
        ));

        let arrays = r#"{"name": "f", "dtype": "f64", "shape": [2], "offset": 8, "nbytes": 16,
                         "unit": "", "description": ""}"#;
        let meta: Metadata = serde_json::from_str(&meta_json(arrays, 16)).expect("json");
        assert!(matches!(
            GoldenSet::from_parts(meta, blob(&[1.0, 2.0])),
            Err(GoldenError::Format { .. })
        ));

        let meta: Metadata = serde_json::from_str(&meta_json("", 8)).expect("json");
        assert!(matches!(
            GoldenSet::from_parts(meta, blob(&[1.0, 2.0])),
            Err(GoldenError::Format { .. })
        ));
    }

    #[test]
    fn loads_every_committed_set() {
        let mut n = 0;
        for entry in std::fs::read_dir(fixtures_dir()).expect("fixtures/golden exists") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("utf-8 name");
                let set = GoldenSet::load(stem).unwrap_or_else(|e| panic!("{e}"));
                for a in &set.meta().arrays {
                    match a.dtype {
                        Dtype::F64 => assert_eq!(set.f64(&a.name).expect("f64").len(), a.len()),
                        Dtype::C128 => {
                            assert_eq!(set.c128(&a.name).expect("c128").len(), a.len())
                        }
                    }
                }
                n += 1;
            }
        }
        assert!(n >= 4, "expected at least 4 golden sets, found {n}");
    }
}
