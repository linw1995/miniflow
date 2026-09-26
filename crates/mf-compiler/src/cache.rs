use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const LAYOUT_VERSION: u32 = 1;
const OWNER: &str = ".mf-owner.json";

#[derive(Debug, Snafu)]
pub enum CacheError {
    #[snafu(display("could not access build directory {path:?}: {source}"))]
    Io { path: PathBuf, source: io::Error },
    #[snafu(display(
        "cannot reuse build directory {path:?}: {reason}; select an empty or compatible directory"
    ))]
    Ownership { path: PathBuf, reason: String },
    #[snafu(display("could not determine the per-user cache directory; use --build-dir"))]
    MissingRoot,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Owner {
    definition: PathBuf,
    cli_version: String,
    layout_version: u32,
}

pub struct BuildDirectory {
    pub path: PathBuf,
    pub reused: bool,
}

pub fn default_build_directory(cache_root: &Path, definition: &Path) -> PathBuf {
    // The ownership marker checks the full identity before any cached files are reused.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in definition
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .copied()
        .chain(env!("CARGO_PKG_VERSION").bytes())
        .chain(LAYOUT_VERSION.to_le_bytes())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    cache_root
        .join("miniflow")
        .join("builds")
        .join(format!("{hash:016x}"))
}

fn cache_root() -> Result<PathBuf, CacheError> {
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join("Library/Caches"));
    }
    #[cfg(windows)]
    if let Some(root) = env::var_os("LOCALAPPDATA") {
        return Ok(root.into());
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(root) = env::var_os("XDG_CACHE_HOME").filter(|p| Path::new(p).is_absolute()) {
            return Ok(root.into());
        }
        if let Some(home) = env::var_os("HOME") {
            return Ok(PathBuf::from(home).join(".cache"));
        }
    }
    MissingRootSnafu.fail()
}

impl BuildDirectory {
    pub fn open(definition: &Path, explicit: Option<&Path>) -> Result<Self, CacheError> {
        let path = match explicit {
            Some(path) => path.to_owned(),
            None => default_build_directory(&cache_root()?, definition),
        };
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .context(IoSnafu { path: path.clone() })?;
        let path = fs::canonicalize(&path).context(IoSnafu { path: path.clone() })?;
        let expected = Owner {
            definition: definition.to_owned(),
            cli_version: env!("CARGO_PKG_VERSION").into(),
            layout_version: LAYOUT_VERSION,
        };
        let marker = path.join(OWNER);
        let reused = match fs::read(&marker) {
            Ok(bytes) => {
                let actual: Owner =
                    serde_json::from_slice(&bytes).map_err(|error| CacheError::Ownership {
                        path: path.clone(),
                        reason: error.to_string(),
                    })?;
                if actual != expected {
                    return OwnershipSnafu {
                        path,
                        reason: "definition, CLI version, or project layout differs",
                    }
                    .fail();
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::read_dir(&path)
                    .context(IoSnafu { path: path.clone() })?
                    .next()
                    .is_some()
                {
                    return OwnershipSnafu {
                        path,
                        reason: "nonempty directory has no ownership marker",
                    }
                    .fail();
                }
                fs::write(&marker, serde_json::to_vec(&expected).unwrap())
                    .context(IoSnafu { path: marker })?;
                false
            }
            Err(source) => {
                return Err(CacheError::Io {
                    path: marker,
                    source,
                });
            }
        };
        Ok(Self { path, reused })
    }
}
