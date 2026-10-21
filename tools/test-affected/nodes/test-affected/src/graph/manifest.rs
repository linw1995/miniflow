use crate::graph::error::{Error, IoSnafu, JsonSnafu};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One per-test coverage export listing consumed by
/// [`GraphWriter::index`](crate::graph::GraphWriter::index).
///
/// JSON shape:
///
/// ```json
/// {
///   "commit_sha": "40-hex commit the tests ran on",
///   "run_id": "producing workflow run id",
///   "attempt": 1,
///   "signature": "normalized JSON text, see module documentation",
///   "tests": [
///     {
///       "package": "mf-runtime",
///       "binary": "mf-runtime",
///       "name": "node::tests::limits_descriptor_depth",
///       "lcov": "per-test-lcov/mf-runtime__node__tests__x.lcov"
///     }
///   ]
/// }
/// ```
///
/// Relative `lcov` paths resolve against the directory containing the
/// manifest when loaded through [`CoverageManifest::read`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageManifest {
    pub commit_sha: String,
    pub run_id: String,
    pub attempt: u32,
    pub signature: String,
    pub tests: Vec<CoveredTest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoveredTest {
    pub package: String,
    pub binary: String,
    pub name: String,
    pub lcov: PathBuf,
}

impl CoverageManifest {
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).context(IoSnafu {
            action: "read coverage manifest",
            path,
        })?;
        let mut manifest: Self = serde_json::from_str(&text).context(JsonSnafu {
            action: "decode coverage manifest",
        })?;
        let base = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for test in &mut manifest.tests {
            if test.lcov.is_relative() {
                test.lcov = base.join(&test.lcov);
            }
        }
        Ok(manifest)
    }
}
