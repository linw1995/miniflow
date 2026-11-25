//! Bounded diagnostic tails; spill original bytes only when a stream no longer fits.

use crate::error::*;
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    io::ioctl_fionread,
};
use snafu::{OptionExt, ResultExt};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::fd::AsFd,
};
use tempfile::NamedTempFile;

const CHUNK: usize = 8192;
const MAX_LINES: usize = 2000;

pub struct Capture<R> {
    pipe: R,
    tail: VecDeque<u8>,
    log: Option<NamedTempFile>,
    limit: usize,
    newlines: usize,
    /// Bytes received before truncation, including data already spilled.
    pub bytes: u64,
}

impl<R: Read + AsFd> Capture<R> {
    pub fn new(pipe: R, limit: usize) -> Result<Self, CodingError> {
        fcntl_getfl(&pipe)
            .and_then(|flags| fcntl_setfl(&pipe, flags | OFlags::NONBLOCK))
            .map_err(io::Error::from)
            .context(IoSnafu {
                operation: "configure command output pipe",
            })?;
        Ok(Self {
            pipe,
            tail: VecDeque::new(),
            log: None,
            limit,
            newlines: 0,
            bytes: 0,
        })
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), CodingError> {
        self.bytes += bytes.len() as u64;
        self.newlines += bytes.iter().filter(|&&b| b == b'\n').count();
        if self.log.is_none()
            && (self.bytes > self.limit as u64
                || self.newlines + usize::from(!bytes.ends_with(b"\n")) > MAX_LINES)
        {
            let mut log = tempfile::Builder::new()
                .prefix("mf-bash-")
                .suffix(".log")
                .tempfile()
                .context(IoSnafu {
                    operation: "create private command log",
                })?;
            for part in [self.tail.as_slices().0, self.tail.as_slices().1] {
                log.write_all(part).context(IoSnafu {
                    operation: "spill command output",
                })?;
            }
            self.log = Some(log);
        }
        if let Some(log) = &mut self.log {
            log.write_all(bytes).context(IoSnafu {
                operation: "write command log",
            })?;
        }
        // One lookbehind byte distinguishes a line boundary from a split first line.
        let keep = self.limit + 1;
        let suffix = &bytes[bytes.len().saturating_sub(keep)..];
        self.tail
            .drain(..(self.tail.len() + suffix.len()).saturating_sub(keep));
        self.tail.extend(suffix);
        Ok(())
    }

    pub fn drain(&mut self, mut budget: u64) -> Result<(), CodingError> {
        let mut buffer = [0; CHUNK];
        while budget > 0 {
            let n = budget.min(CHUNK as u64) as usize;
            match self.pipe.read(&mut buffer[..n]) {
                Ok(0) => break,
                Ok(n) => {
                    self.append(&buffer[..n])?;
                    budget -= n as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(error).context(IoSnafu {
                        operation: "read command output",
                    });
                }
            }
        }
        Ok(())
    }

    pub fn drain_remaining(&mut self) -> Result<(), CodingError> {
        // The owned group has stopped. A detached descendant may still hold a pipe:
        // capture everything already buffered without waiting for EOF or more writes.
        let pending = ioctl_fionread(&self.pipe)
            .map_err(io::Error::from)
            .context(IoSnafu {
                operation: "measure buffered command output",
            })?;
        self.drain(pending)
    }

    pub fn preview(&mut self) -> String {
        tail_text(self.tail.make_contiguous(), self.limit.min(2048), 12)
    }

    pub fn finish(mut self) -> Result<(String, Option<String>), CodingError> {
        let text = tail_text(self.tail.make_contiguous(), self.limit, MAX_LINES);
        let path = self
            .log
            .map(|mut log| {
                let path = std::path::absolute(log.path()).context(IoSnafu {
                    operation: "resolve command log path",
                })?;
                let path =
                    path.into_os_string()
                        .into_string()
                        .ok()
                        .context(InvalidConfigSnafu {
                            field: "temporary directory",
                            requirement: "a UTF-8 path for command logs",
                        })?;
                log.disable_cleanup(true);
                Ok::<_, CodingError>(path)
            })
            .transpose()?;
        Ok((text, path))
    }
}

fn tail_text(bytes: &[u8], limit: usize, max_lines: usize) -> String {
    let mut start = bytes.len().saturating_sub(limit);
    if start > 0 && bytes[start - 1] != b'\n' {
        if let Some(end) = bytes[start..].iter().position(|&b| b == b'\n')
            && start + end + 1 < bytes.len()
        {
            start += end + 1;
        } else {
            // A single oversized final line still has a useful suffix.
            while start < bytes.len() && bytes[start] & 0xc0 == 0x80 {
                start += 1;
            }
        }
    }
    let suffix = &bytes[start..];
    let lines = suffix.strip_suffix(b"\n").unwrap_or(suffix);
    if let Some((cut, _)) = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, b)| **b == b'\n')
        .nth(max_lines - 1)
    {
        start += cut + 1;
    }
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostic_tails_prefer_complete_lines_and_do_not_split_valid_utf8() {
        for (input, limit, lines, expected) in [
            ("exact\nfit\n", 10, 2, "exact\nfit\n"),
            ("prefix\nmiddle\nlast", 9, 2, "last"),
            ("a\nb\nc\n", 8, 2, "b\nc\n"),
            ("a\nb\nc", 8, 2, "b\nc"),
            ("prefixé/end\n", 6, 2, "/end\n"),
            ("prefix😀", 4, 1, "😀"),
            ("prefix😀", 1, 1, ""),
        ] {
            assert_eq!(super::tail_text(input.as_bytes(), limit, lines), expected);
        }
    }
}
