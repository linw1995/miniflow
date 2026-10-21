use crate::graph::{
    LineRange,
    error::{Error, IoSnafu, LcovNumberSnafu, LcovSnafu},
};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    fs,
    path::{Component, Path},
};

/// Parses one per-test lcov export into workspace-relative hit intervals.
///
/// Only `DA` records with a positive hit count contribute; consecutive hit
/// lines compress into one inclusive interval. Source files outside
/// `workspace_root` are ignored because llvm-cov exports dependency sources
/// too. Relative `SF` paths are already workspace-relative.
pub(super) fn parse(
    path: &Path,
    workspace_root: &Path,
) -> Result<Vec<(String, Vec<LineRange>)>, Error> {
    let text = fs::read_to_string(path).context(IoSnafu {
        action: "read lcov export",
        path,
    })?;
    let mut parsed = Vec::new();
    let mut source: Option<String> = None;
    let mut hits: Vec<u32> = Vec::new();
    for line in text.lines() {
        if let Some(file) = line.strip_prefix("SF:") {
            if let Some(file) = source.take() {
                let hits = std::mem::take(&mut hits);
                if !hits.is_empty() {
                    parsed.push((file, compress(hits)));
                }
            }
            source = workspace_path(file.trim(), workspace_root, path)?;
        } else if let Some(record) = line.strip_prefix("DA:")
            && source.is_some()
        {
            let mut fields = record.split(',');
            let (Some(number), Some(count)) = (fields.next(), fields.next()) else {
                return LcovSnafu { record: "DA", path }.fail();
            };
            let number: u32 = number
                .trim()
                .parse()
                .context(LcovNumberSnafu { record: "DA", path })?;
            let count: u64 = count
                .trim()
                .parse()
                .context(LcovNumberSnafu { record: "DA", path })?;
            ensure!(number >= 1, LcovSnafu { record: "DA", path });
            if count > 0 {
                hits.push(number);
            }
        }
    }
    if let Some(file) = source {
        let hits = std::mem::take(&mut hits);
        if !hits.is_empty() {
            parsed.push((file, compress(hits)));
        }
    }
    Ok(parsed)
}

fn workspace_path(
    source: &str,
    workspace_root: &Path,
    lcov: &Path,
) -> Result<Option<String>, Error> {
    let path = Path::new(source);
    let relative = if path.is_absolute() {
        match path.strip_prefix(workspace_root) {
            Ok(relative) => relative,
            Err(_) => return Ok(None),
        }
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Ok(None);
    }
    let relative = relative.to_str().context(LcovSnafu {
        record: "SF",
        path: lcov,
    })?;
    Ok(Some(relative.to_owned()))
}

fn compress(mut hits: Vec<u32>) -> Vec<LineRange> {
    hits.sort_unstable();
    hits.dedup();
    let mut ranges: Vec<LineRange> = Vec::new();
    for line in hits {
        match ranges.last_mut() {
            Some(last) if line == last.end + 1 => last.end = line,
            _ => ranges.push(LineRange {
                start: line,
                end: line,
            }),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compress_lines(lines: &[u32]) -> Vec<LineRange> {
        compress(lines.to_vec())
    }

    #[test]
    fn compression_merges_consecutive_lines_and_splits_gaps() {
        assert_eq!(
            compress_lines(&[3, 1, 2, 2, 10, 12, 13]),
            vec![
                LineRange { start: 1, end: 3 },
                LineRange { start: 10, end: 10 },
                LineRange { start: 12, end: 13 },
            ]
        );
        assert!(compress_lines(&[]).is_empty());
    }

    #[test]
    fn workspace_paths_strip_the_root_and_reject_escapes() {
        let root = Path::new("/ws");
        let lcov = Path::new("export.lcov");
        assert_eq!(
            workspace_path("/ws/crates/core/src/lib.rs", root, lcov).unwrap(),
            Some("crates/core/src/lib.rs".into())
        );
        assert_eq!(
            workspace_path("crates/core/src/lib.rs", root, lcov).unwrap(),
            Some("crates/core/src/lib.rs".into())
        );
        assert_eq!(
            workspace_path("/other/src/lib.rs", root, lcov).unwrap(),
            None
        );
        assert_eq!(workspace_path("../outside.rs", root, lcov).unwrap(), None);
        assert_eq!(workspace_path("/ws", root, lcov).unwrap(), None);
    }
}
