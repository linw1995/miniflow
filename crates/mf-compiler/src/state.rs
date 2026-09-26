use snafu::{ResultExt, Snafu};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Snafu)]
pub enum StateError {
    #[snafu(display("could not update build state {path:?}: {source}"))]
    Io { path: PathBuf, source: io::Error },
    #[snafu(display(
        "build state is busy or cannot be locked at {path:?}; retry after the active build exits: {message}"
    ))]
    Lock { path: PathBuf, message: String },
}

pub struct BuildGuard {
    _file: File,
}
impl BuildGuard {
    pub fn acquire(path: &Path) -> Result<Self, StateError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).context(IoSnafu {
            path: path.to_owned(),
        })?;
        file.try_lock().map_err(|error| StateError::Lock {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
        Ok(Self { _file: file })
    }
}

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
struct StagedFile {
    path: PathBuf,
    file: File,
}
impl StagedFile {
    fn new(destination: &Path) -> Result<Self, StateError> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        for _ in 0..128 {
            let path = parent.join(format!(
                ".mf-stage-{}-{}",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(StateError::Io { path, source }),
            }
        }
        Err(StateError::Io {
            path: destination.to_owned(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not reserve a staging file",
            ),
        })
    }
    fn install(&self, destination: &Path) -> Result<(), StateError> {
        self.file.sync_all().context(IoSnafu {
            path: self.path.clone(),
        })?;
        fs::rename(&self.path, destination).context(IoSnafu {
            path: destination.to_owned(),
        })
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn atomic_write(destination: &Path, contents: &[u8]) -> Result<(), StateError> {
    let mut staged = StagedFile::new(destination)?;
    staged.file.write_all(contents).context(IoSnafu {
        path: staged.path.clone(),
    })?;
    staged.install(destination)
}

pub fn atomic_copy(source: &Path, destination: &Path) -> Result<(), StateError> {
    let staged = StagedFile::new(destination)?;
    fs::copy(source, &staged.path).context(IoSnafu {
        path: source.to_owned(),
    })?;
    staged.install(destination)
}
