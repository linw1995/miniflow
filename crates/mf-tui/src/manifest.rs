use mf_runtime::{MAX_MANIFEST_SECTION_BYTES, WorkflowManifest, WorkflowManifestError};
use object::{
    Endianness, FileKind,
    read::{
        ReadCache, ReadRef,
        elf::{FileHeader, ProgramHeader, SectionHeader},
        macho::{MachHeader, Section, Segment},
    },
};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

const MAX_METADATA_BYTES: usize = 1024 * 1024;
const MAX_METADATA_READS: usize = 8192;
const MAX_SECTION_NAME_BYTES: u64 = 1024;
const MAX_SECTIONS: usize = 4096;

#[derive(Debug, Snafu)]
pub enum ManifestReadError {
    #[snafu(display("could not read workflow executable {path:?}: {source}"))]
    Io { path: PathBuf, source: io::Error },
    #[snafu(display("workflow executable {path:?} must be a regular file"))]
    NotRegular { path: PathBuf },
    #[snafu(display(
        "unsupported workflow executable format; use a Linux ELF64 or macOS Mach-O64 runner"
    ))]
    UnsupportedContainer,
    #[snafu(display("invalid workflow executable: {source}; recompile the workflow"))]
    Container { source: object::read::Error },
    #[snafu(display("workflow executable metadata exceeds inspection limits"))]
    MetadataLimit,
    #[snafu(display("workflow manifest section is ambiguous or has an invalid type or segment"))]
    InvalidSection,
    #[snafu(display("workflow executable range {offset}+{size} is outside its file"))]
    InvalidRange { offset: u64, size: u64 },
    #[snafu(display("workflow manifest section exceeds the {limit}-byte limit"))]
    TooLarge { limit: usize },
    #[snafu(display("invalid embedded workflow manifest: {source}; recompile the workflow"))]
    Manifest { source: WorkflowManifestError },
}

struct FileSource {
    file: File,
    length: u64,
    failure: Rc<RefCell<Option<io::Error>>>,
}

impl FileSource {
    // The object cache erases I/O causes, so keep the original error for the caller.
    fn capture<T>(&self, result: io::Result<T>) -> Result<T, ()> {
        match result {
            Ok(value) => Ok(value),
            Err(source) => {
                *self.failure.borrow_mut() = Some(source);
                Err(())
            }
        }
    }
}

impl object::read::ReadCacheOps for FileSource {
    fn len(&mut self) -> Result<u64, ()> {
        Ok(self.length)
    }
    fn seek(&mut self, pos: u64) -> Result<u64, ()> {
        let result = Seek::seek(&mut self.file, SeekFrom::Start(pos));
        self.capture(result)
    }
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        let result = Read::read(&mut self.file, buf);
        self.capture(result)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ()> {
        let result = Read::read_exact(&mut self.file, buf);
        self.capture(result)
    }
}

#[derive(Clone, Copy)]
struct Metadata<'a> {
    cache: &'a ReadCache<FileSource>,
    bytes: &'a Cell<usize>,
    reads: &'a Cell<usize>,
    exceeded: &'a Cell<bool>,
    length: u64,
}

impl<'a> ReadRef<'a> for Metadata<'a> {
    fn len(self) -> Result<u64, ()> {
        Ok(self.length)
    }

    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'a [u8], ()> {
        if size > self.bytes.get() as u64 || self.reads.get() == 0 {
            self.exceeded.set(true);
            return Err(());
        }
        if offset.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(());
        }
        self.bytes.set(self.bytes.get() - size as usize);
        self.reads.set(self.reads.get() - 1);
        self.cache.read_bytes_at(offset, size)
    }

    fn read_bytes_at_until(self, range: Range<u64>, delimiter: u8) -> Result<&'a [u8], ()> {
        if range.start > range.end || range.end > self.length {
            return Err(());
        }
        let bytes = self.read_bytes_at(
            range.start,
            (range.end - range.start).min(MAX_SECTION_NAME_BYTES),
        )?;
        let end = bytes.iter().position(|byte| *byte == delimiter).ok_or(())?;
        Ok(&bytes[..end])
    }
}

fn check_range(length: u64, offset: u64, size: u64) -> Result<(), ManifestReadError> {
    ensure!(
        offset.checked_add(size).is_some_and(|end| end <= length),
        InvalidRangeSnafu { offset, size }
    );
    Ok(())
}

fn select_range(
    selected: &mut Option<(u64, u64)>,
    range: Option<(u64, u64)>,
) -> Result<(), ManifestReadError> {
    ensure!(selected.is_none() && range.is_some(), InvalidSectionSnafu);
    *selected = range;
    Ok(())
}

fn elf_range(data: Metadata<'_>) -> Result<Option<(u64, u64)>, ManifestReadError> {
    let header = object::elf::FileHeader64::<Endianness>::parse(data).context(ContainerSnafu)?;
    let endian = header.endian().context(ContainerSnafu)?;
    ensure!(
        matches!(
            header.e_type(endian),
            object::elf::ET_EXEC | object::elf::ET_DYN
        ),
        UnsupportedContainerSnafu
    );
    for segment in header
        .program_headers(endian, data)
        .context(ContainerSnafu)?
    {
        check_range(
            data.length,
            segment.p_offset(endian),
            segment.p_filesz(endian),
        )?;
    }
    let sections = header.sections(endian, data).context(ContainerSnafu)?;
    ensure!(sections.len() <= MAX_SECTIONS, MetadataLimitSnafu);
    let mut selected = None;
    for section in sections.iter() {
        if let Some((offset, size)) = section.file_range(endian) {
            check_range(data.length, offset, size)?;
        }
        if sections
            .section_name(endian, section)
            .context(ContainerSnafu)?
            == b".mf_manifest"
        {
            ensure!(
                section.sh_type(endian) == object::elf::SHT_PROGBITS,
                InvalidSectionSnafu
            );
            select_range(&mut selected, section.file_range(endian))?;
        }
    }
    Ok(selected)
}

fn macho_range(data: Metadata<'_>) -> Result<Option<(u64, u64)>, ManifestReadError> {
    let header =
        object::macho::MachHeader64::<Endianness>::parse(data, 0).context(ContainerSnafu)?;
    let endian = header.endian().context(ContainerSnafu)?;
    ensure!(
        header.filetype(endian) == object::macho::MH_EXECUTE,
        UnsupportedContainerSnafu
    );
    ensure!(
        header.ncmds(endian) as usize <= MAX_SECTIONS,
        MetadataLimitSnafu
    );
    let mut commands = header
        .load_commands(endian, data, 0)
        .context(ContainerSnafu)?;
    let mut selected = None;
    let mut count = 0usize;
    while let Some(command) = commands.next().context(ContainerSnafu)? {
        if let Some((segment, section_data)) = command.segment_64().context(ContainerSnafu)? {
            let (segment_offset, segment_size) = segment.file_range(endian);
            check_range(data.length, segment_offset, segment_size)?;
            let sections = segment
                .sections(endian, section_data)
                .context(ContainerSnafu)?;
            count += sections.len();
            ensure!(count <= MAX_SECTIONS, MetadataLimitSnafu);
            for section in segment.section_offsets(endian, sections) {
                let (section, offset) = section.context(ContainerSnafu)?;
                if let Some((offset, size)) = section.file_range(endian, offset) {
                    check_range(data.length, offset, size)?;
                }
                if section.name() == b"__mf_manifest" {
                    ensure!(
                        segment.name() == b"__DATA" && section.segment_name() == b"__DATA",
                        InvalidSectionSnafu
                    );
                    let range = section.file_range(endian, offset);
                    if let Some((offset, size)) = range {
                        ensure!(
                            offset >= segment_offset
                                && offset
                                    .checked_add(size)
                                    .is_some_and(|end| end <= segment_offset + segment_size),
                            InvalidRangeSnafu { offset, size }
                        );
                    }
                    select_range(&mut selected, range)?;
                }
            }
        }
    }
    Ok(selected)
}

/// Returns None only for a recognized executable with no manifest section.
pub fn read_manifest(path: &Path) -> Result<Option<WorkflowManifest>, ManifestReadError> {
    let file = File::options()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
        .context(IoSnafu {
            path: path.to_owned(),
        })?;
    let metadata = file.metadata().context(IoSnafu {
        path: path.to_owned(),
    })?;
    ensure!(
        metadata.is_file(),
        NotRegularSnafu {
            path: path.to_owned()
        }
    );
    let length = metadata.len();
    let failure = Rc::new(RefCell::new(None));
    let cache = ReadCache::new(FileSource {
        file,
        length,
        failure: failure.clone(),
    });
    let bytes = Cell::new(MAX_METADATA_BYTES);
    let reads = Cell::new(MAX_METADATA_READS);
    let exceeded = Cell::new(false);
    let data = Metadata {
        cache: &cache,
        bytes: &bytes,
        reads: &reads,
        exceeded: &exceeded,
        length,
    };
    let range = (|| match FileKind::parse(data).context(ContainerSnafu)? {
        FileKind::Elf64 => elf_range(data),
        FileKind::MachO64 => macho_range(data),
        _ => UnsupportedContainerSnafu.fail(),
    })();
    if let Some(source) = failure.borrow_mut().take() {
        return Err(source).context(IoSnafu {
            path: path.to_owned(),
        });
    }
    ensure!(!exceeded.get(), MetadataLimitSnafu);
    let Some((offset, size)) = range? else {
        return Ok(None);
    };
    ensure!(
        size <= MAX_MANIFEST_SECTION_BYTES as u64,
        TooLargeSnafu {
            limit: MAX_MANIFEST_SECTION_BYTES
        }
    );
    check_range(length, offset, size)?;
    let mut file = cache.into_inner().file;
    file.seek(SeekFrom::Start(offset)).context(IoSnafu {
        path: path.to_owned(),
    })?;
    let mut payload = vec![0; size as usize];
    file.read_exact(&mut payload).context(IoSnafu {
        path: path.to_owned(),
    })?;
    Ok(Some(
        WorkflowManifest::from_bytes(&payload).context(ManifestSnafu)?,
    ))
}
