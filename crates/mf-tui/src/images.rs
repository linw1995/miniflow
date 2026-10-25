//! Bounded background image loading and cell-based terminal graphics.

use image::{DynamicImage, ImageReader, Limits};
use ratatui::{
    Frame,
    layout::{Rect, Size},
    style::{Color, Style},
    text::Line,
};
use ratatui_image::{
    FontSize, Resize,
    picker::{Picker, ProtocolType, cap_parser::Parser},
    protocol::kitty::Kitty,
    sliced::{SlicedImage, SlicedProtocol},
};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Weak,
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};
use url::Url;

const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_COLUMNS: u16 = 120;
const MAX_ROWS: u16 = 40;
static NEXT_IMAGE_ID: LazyLock<AtomicU32> =
    LazyLock::new(|| AtomicU32::new(std::process::id().wrapping_mul(0x9e37_79b9).max(1)));

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub source: String,
    pub alt: String,
}

impl ImageRef {
    fn label(&self) -> &str {
        if self.alt.is_empty() {
            &self.source
        } else {
            &self.alt
        }
    }
}

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("{operation}: {source}"))]
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[snafu(transparent)]
    Http { source: reqwest::Error },
    #[snafu(transparent)]
    Url { source: url::ParseError },
    #[snafu(transparent)]
    Decode { source: image::ImageError },
    #[snafu(transparent)]
    Encode {
        source: ratatui_image::errors::Errors,
    },
    #[snafu(display("{message}"))]
    Resource { message: &'static str },
}

#[derive(Clone, Copy, Debug)]
struct Spec {
    width: u16,
    font: FontSize,
}

impl PartialEq for Spec {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.font.width == other.font.width
            && self.font.height == other.font.height
    }
}

impl Eq for Spec {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    source: String,
    width: u16,
    font_width: u16,
    font_height: u16,
}

impl Key {
    fn new(source: &str, spec: Spec) -> Self {
        Self {
            source: source.to_owned(),
            width: spec.width,
            font_width: spec.font.width,
            font_height: spec.font.height,
        }
    }
}

struct Encoded {
    spec: Spec,
    protocol: SlicedProtocol,
    kitty_id: Option<u32>,
    tmux: bool,
    bytes: usize,
}

struct Loaded {
    original: Arc<DynamicImage>,
    encoded: Encoded,
}

enum State {
    Waiting,
    Loading(u64),
    Ready,
    Failed(String),
}

struct Asset {
    original: Option<Arc<DynamicImage>>,
    encoded: Option<Encoded>,
    spec: Spec,
    state: State,
    last_used: u64,
}

struct Work {
    key: Key,
    id: u64,
    epoch: u64,
    original: Option<Arc<DynamicImage>>,
    spec: Spec,
    picker: Picker,
}

struct Completed {
    key: Key,
    id: u64,
    result: Result<Loaded, Error>,
}

struct Worker {
    requests: SyncSender<Work>,
    results: Receiver<Completed>,
    epoch: Arc<AtomicU64>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Presentation {
    pub height: usize,
    pub caption: Line<'static>,
    pub ready: bool,
}

#[derive(Default)]
pub struct Images {
    picker: Option<Picker>,
    worker: Option<Worker>,
    assets: BTreeMap<Key, Asset>,
    retired: Vec<(u32, bool)>,
    next_request: u64,
    clock: u64,
    revision: u64,
    visible: BTreeSet<Key>,
}

impl Images {
    pub fn enable(&mut self, root: PathBuf, picker: Picker) -> io::Result<()> {
        self.clear();
        let (requests, jobs) = mpsc::sync_channel::<Work>(8);
        let (results, responses) = mpsc::sync_channel(2);
        let epoch = Arc::new(AtomicU64::new(0));
        let worker_epoch = epoch.clone();
        thread::Builder::new()
            .name("chat-images".into())
            .spawn(move || {
                let mut client = None;
                let mut originals = BTreeMap::<String, Weak<DynamicImage>>::new();
                let mut current_epoch = 0;
                while let Ok(job) = jobs.recv() {
                    if worker_epoch.load(Ordering::Relaxed) != job.epoch {
                        continue;
                    }
                    if current_epoch != job.epoch {
                        originals.clear();
                        current_epoch = job.epoch;
                    }
                    let result = load(&root, &job, &mut client, &worker_epoch, &mut originals);
                    if worker_epoch.load(Ordering::Relaxed) != job.epoch {
                        continue;
                    }
                    if results
                        .send(Completed {
                            key: job.key,
                            id: job.id,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        self.worker = Some(Worker {
            requests,
            results: responses,
            epoch,
        });
        self.picker = Some(picker);
        Ok(())
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn begin_frame(&mut self) {
        self.visible.clear();
        self.poll();
    }

    pub fn update_picker(&mut self, picker: Picker) {
        if self.picker.as_ref().is_some_and(|old| {
            old.font_size().width == picker.font_size().width
                && old.font_size().height == picker.font_size().height
                && old.protocol_type() == picker.protocol_type()
        }) {
            return;
        }
        if self
            .picker
            .as_ref()
            .is_some_and(|old| old.protocol_type() != picker.protocol_type())
        {
            self.clear();
        }
        self.picker = Some(picker);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn poll(&mut self) {
        self.clock = self.clock.saturating_add(1);
        while let Some(completed) = self
            .worker
            .as_ref()
            .and_then(|worker| worker.results.try_recv().ok())
        {
            let Some(asset) = self.assets.get(&completed.key) else {
                continue;
            };
            if !matches!(asset.state, State::Loading(id) if id == completed.id) {
                continue;
            }
            match completed.result {
                Ok(loaded) => {
                    if !self.reserve(&loaded) {
                        self.assets.get_mut(&completed.key).unwrap().state =
                            State::Failed("image cache limit reached".into());
                        self.revision = self.revision.saturating_add(1);
                        continue;
                    }
                    let asset = self.assets.get_mut(&completed.key).unwrap();
                    if let Some(encoded) = asset.encoded.take() {
                        retire(&mut self.retired, encoded);
                    }
                    asset.original = Some(loaded.original);
                    asset.encoded = Some(loaded.encoded);
                    asset.state = State::Ready;
                }
                Err(error) => {
                    self.assets.get_mut(&completed.key).unwrap().state =
                        State::Failed(error.to_string())
                }
            }
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn prepare(&mut self, image: &ImageRef, width: u16) -> Presentation {
        let fallback = || {
            Line::styled(
                format!("[image] {}", crate::markdown::safe(image.label())),
                Style::default().fg(Color::DarkGray),
            )
        };
        let Some(picker) = &self.picker else {
            return Presentation {
                height: 1,
                caption: fallback(),
                ready: false,
            };
        };
        let spec = Spec {
            width: width.clamp(1, MAX_COLUMNS),
            font: picker.font_size(),
        };
        let Some(asset) = self.assets.get(&Key::new(&image.source, spec)) else {
            let height = self
                .assets
                .iter()
                .filter(|(key, _)| key.source == image.source)
                .find_map(|(_, asset)| asset.original.as_ref())
                .map_or(1, |original| desired_size(original, spec).height as usize);
            return Presentation {
                height,
                caption: Line::styled(
                    format!("[image loading] {}", crate::markdown::safe(image.label())),
                    Style::default().fg(Color::DarkGray),
                ),
                ready: false,
            };
        };
        let height = asset
            .original
            .as_ref()
            .map_or(1, |original| desired_size(original, spec).height as usize);
        match &asset.state {
            State::Ready => Presentation {
                height,
                caption: fallback(),
                ready: true,
            },
            State::Failed(error) => Presentation {
                height: 1,
                caption: Line::styled(
                    format!(
                        "[image failed] {}: {}",
                        crate::markdown::safe(image.label()),
                        crate::markdown::safe(error)
                    ),
                    Style::default().fg(Color::Yellow),
                ),
                ready: false,
            },
            _ => Presentation {
                height,
                caption: Line::styled(
                    format!("[image loading] {}", crate::markdown::safe(image.label())),
                    Style::default().fg(Color::DarkGray),
                ),
                ready: false,
            },
        }
    }

    pub fn request(&mut self, image: &ImageRef, width: u16) {
        let Some(picker) = &self.picker else {
            return;
        };
        let spec = Spec {
            width: width.clamp(1, MAX_COLUMNS),
            font: picker.font_size(),
        };
        let key = Key::new(&image.source, spec);
        self.visible.insert(key.clone());
        if !self.assets.contains_key(&key) {
            let original = self
                .assets
                .iter()
                .filter(|(key, _)| key.source == image.source)
                .find_map(|(_, asset)| asset.original.clone());
            while self.assets.len() >= MAX_ENTRIES {
                if !self.evict_oldest() {
                    return;
                }
            }
            self.assets.insert(
                key.clone(),
                Asset {
                    original,
                    encoded: None,
                    spec,
                    state: State::Waiting,
                    last_used: self.clock,
                },
            );
        }
        let asset = self.assets.get_mut(&key).unwrap();
        asset.last_used = self.clock;
        if matches!(asset.state, State::Waiting)
            && let (Some(worker), Some(picker)) = (&self.worker, &self.picker)
        {
            self.next_request = self.next_request.saturating_add(1);
            let job = Work {
                key,
                id: self.next_request,
                epoch: worker.epoch.load(Ordering::Relaxed),
                original: asset.original.clone(),
                spec,
                picker: picker.clone(),
            };
            if worker.requests.try_send(job).is_ok() {
                asset.state = State::Loading(self.next_request);
            }
        }
    }

    pub fn render(
        &self,
        image: &ImageRef,
        width: u16,
        frame: &mut Frame,
        area: Rect,
        skip: usize,
    ) -> bool {
        let Some(picker) = &self.picker else {
            return false;
        };
        let spec = Spec {
            width: width.clamp(1, MAX_COLUMNS),
            font: picker.font_size(),
        };
        let Some(asset) = self.assets.get(&Key::new(&image.source, spec)) else {
            return false;
        };
        let Some(encoded) = asset
            .encoded
            .as_ref()
            .filter(|encoded| encoded.spec == asset.spec)
        else {
            return false;
        };
        if !matches!(asset.state, State::Ready) || area.is_empty() {
            return false;
        }
        frame.render_widget(
            SlicedImage::new(&encoded.protocol, (0, -(skip as i16)).into()),
            area,
        );
        true
    }

    pub fn clear(&mut self) {
        self.visible.clear();
        if let Some(worker) = &self.worker {
            worker.epoch.fetch_add(1, Ordering::Relaxed);
        }
        for (_, asset) in std::mem::take(&mut self.assets) {
            if let Some(encoded) = asset.encoded {
                retire(&mut self.retired, encoded);
            }
        }
        self.revision = self.revision.saturating_add(1);
    }

    pub fn flush_retired(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        for (id, tmux) in self.retired.drain(..) {
            let (start, escape, end) = Parser::tmux_start_escape_end(tmux);
            write!(writer, "{start}{escape}_Ga=d,d=I,i={id},q=2{escape}\\{end}")?;
        }
        writer.flush()
    }

    fn cached_bytes(&self) -> usize {
        let mut originals = BTreeSet::new();
        self.assets
            .values()
            .map(|asset| {
                let original = asset
                    .original
                    .as_ref()
                    .filter(|image| originals.insert(Arc::as_ptr(image) as usize))
                    .map_or(0, |image| {
                        image.width() as usize * image.height() as usize * 4
                    });
                original + asset.encoded.as_ref().map_or(0, |encoded| encoded.bytes)
            })
            .sum()
    }

    fn reserve(&mut self, loaded: &Loaded) -> bool {
        loop {
            let shared = self.assets.values().any(|asset| {
                asset
                    .original
                    .as_ref()
                    .is_some_and(|image| Arc::ptr_eq(image, &loaded.original))
            });
            let original = if shared {
                0
            } else {
                loaded.original.width() as usize * loaded.original.height() as usize * 4
            };
            if self.cached_bytes() + original + loaded.encoded.bytes <= MAX_CACHE_BYTES {
                return true;
            }
            if !self.evict_oldest() {
                return false;
            }
        }
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(key) = self
            .assets
            .iter()
            .filter(|(key, asset)| {
                !self.visible.contains(*key) && !matches!(asset.state, State::Loading(_))
            })
            .min_by_key(|(_, asset)| asset.last_used)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        let asset = self.assets.remove(&key).unwrap();
        if let Some(encoded) = asset.encoded {
            retire(&mut self.retired, encoded);
        }
        self.revision = self.revision.saturating_add(1);
        true
    }
}

fn retire(retired: &mut Vec<(u32, bool)>, encoded: Encoded) {
    if let Some(id) = encoded.kitty_id {
        retired.push((id, encoded.tmux));
    }
}

fn desired_size(image: &DynamicImage, spec: Spec) -> Size {
    Resize::Fit(None).size_for(image, spec.font, Size::new(spec.width, MAX_ROWS))
}

fn load(
    root: &Path,
    job: &Work,
    client: &mut Option<reqwest::blocking::Client>,
    epoch: &AtomicU64,
    originals: &mut BTreeMap<String, Weak<DynamicImage>>,
) -> Result<Loaded, Error> {
    let original = if let Some(original) = job
        .original
        .clone()
        .or_else(|| originals.get(&job.key.source).and_then(Weak::upgrade))
    {
        original
    } else {
        Arc::new(decode(read_source(
            root,
            &job.key.source,
            client,
            epoch,
            job.epoch,
        )?)?)
    };
    originals.retain(|_, original| original.strong_count() > 0);
    originals.insert(job.key.source.clone(), Arc::downgrade(&original));
    ensure!(
        epoch.load(Ordering::Relaxed) == job.epoch,
        ResourceSnafu {
            message: "image request cancelled"
        }
    );
    let bounds = Size::new(job.spec.width, MAX_ROWS);
    let size = desired_size(&original, job.spec);
    ensure!(
        u64::from(size.width)
            * u64::from(job.spec.font.width)
            * u64::from(size.height)
            * u64::from(job.spec.font.height)
            <= MAX_PIXELS,
        ResourceSnafu {
            message: "rendered image exceeds the 4 megapixel limit"
        }
    );
    let (protocol, kitty_id, encoded_bytes) = if job.picker.protocol_type() == ProtocolType::Kitty {
        let raster = Resize::Fit(None).resize(&original, job.spec.font, size, None);
        let bytes = raster.width() as usize * raster.height() as usize * 6 + 512;
        let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
        let kitty = Kitty::new(raster, size, id, job.picker.tmux_detected(), false)?;
        (SlicedProtocol::Kitty(kitty), Some(id), bytes)
    } else {
        let protocol = SlicedProtocol::new(&job.picker, (*original).clone(), Some(bounds))?;
        let size = protocol.size();
        let bytes = size.width as usize * size.height as usize * 256;
        (protocol, None, bytes)
    };
    Ok(Loaded {
        original,
        encoded: Encoded {
            spec: job.spec,
            protocol,
            kitty_id,
            tmux: job.picker.tmux_detected(),
            bytes: encoded_bytes,
        },
    })
}

fn read_source(
    root: &Path,
    source: &str,
    client: &mut Option<reqwest::blocking::Client>,
    epoch: &AtomicU64,
    expected_epoch: u64,
) -> Result<Vec<u8>, Error> {
    let base = Url::from_directory_path(root).ok();
    let url = match Url::parse(source) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let Some(base) = base else {
                return ResourceSnafu {
                    message: "image base directory must be absolute",
                }
                .fail();
            };
            base.join(source)?
        }
        Err(source) => return Err(source.into()),
    };
    let mut reader: Box<dyn Read> = match url.scheme() {
        "file" => {
            let Ok(path) = url.to_file_path() else {
                return ResourceSnafu {
                    message: "image URL must name a local file",
                }
                .fail();
            };
            let metadata = std::fs::metadata(&path).context(IoSnafu {
                operation: "inspect image file",
            })?;
            ensure!(
                metadata.is_file(),
                ResourceSnafu {
                    message: "image path must name a regular file"
                }
            );
            ensure!(
                metadata.len() <= MAX_FILE_BYTES as u64,
                ResourceSnafu {
                    message: "image exceeds the 8 MiB download limit"
                }
            );
            Box::new(File::open(path).context(IoSnafu {
                operation: "open image file",
            })?)
        }
        "http" | "https" => {
            if client.is_none() {
                *client = Some(
                    reqwest::blocking::Client::builder()
                        .connect_timeout(Duration::from_secs(2))
                        .timeout(Duration::from_secs(5))
                        .redirect(reqwest::redirect::Policy::limited(5))
                        .build()?,
                );
            }
            let response = client
                .as_ref()
                .unwrap()
                .get(url)
                .send()?
                .error_for_status()?;
            ensure!(
                response
                    .content_length()
                    .is_none_or(|length| length <= MAX_FILE_BYTES as u64),
                ResourceSnafu {
                    message: "image exceeds the 8 MiB download limit"
                }
            );
            Box::new(response)
        }
        _ => {
            return ResourceSnafu {
                message: "image URL scheme is unsupported",
            }
            .fail();
        }
    };
    let mut bytes = vec![];
    let mut chunk = [0u8; 8192];
    loop {
        ensure!(
            epoch.load(Ordering::Relaxed) == expected_epoch,
            ResourceSnafu {
                message: "image request cancelled"
            }
        );
        let count = reader.read(&mut chunk).context(IoSnafu {
            operation: "read image data",
        })?;
        if count == 0 {
            break;
        }
        ensure!(
            bytes.len() + count <= MAX_FILE_BYTES,
            ResourceSnafu {
                message: "image exceeds the 8 MiB download limit"
            }
        );
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}

fn decode(bytes: Vec<u8>) -> Result<DynamicImage, Error> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut header = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .context(IoSnafu {
            operation: "identify image format",
        })?;
    // Header parsing may allocate before pixel decoding starts.
    header.limits(limits.clone());
    let dimensions = header.into_dimensions()?;
    ensure!(
        u64::from(dimensions.0) * u64::from(dimensions.1) <= MAX_PIXELS,
        ResourceSnafu {
            message: "image exceeds the 4 megapixel decode limit"
        }
    );
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context(IoSnafu {
            operation: "identify image format",
        })?;
    reader.limits(limits);
    Ok(DynamicImage::ImageRgba8(reader.decode()?.into_rgba8()))
}

#[expect(
    deprecated,
    reason = "Picker exposes custom cell dimensions only through this constructor; querying stdout would violate the chat output contract."
)]
fn picker_with_size(font: FontSize) -> Picker {
    Picker::from_fontsize(font)
}

pub fn terminal_picker() -> Picker {
    let font = rustix::termios::tcgetwinsize(io::stderr())
        .ok()
        .and_then(|size| {
            if size.ws_col == 0 || size.ws_row == 0 {
                return None;
            }
            let width = size.ws_xpixel / size.ws_col;
            let height = size.ws_ypixel / size.ws_row;
            (width > 0 && height > 0).then_some(FontSize::new(width, height))
        })
        .unwrap_or(FontSize::new(8, 16));
    let mut picker = picker_with_size(font);
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let kitty = program == "ghostty"
        || term == "xterm-ghostty"
        || std::env::var_os("KITTY_WINDOW_ID").is_some();
    picker.set_protocol_type(if kitty {
        ProtocolType::Kitty
    } else {
        ProtocolType::Halfblocks
    });
    picker
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use ratatui::{Terminal, backend::TestBackend};
    use std::{net::TcpListener, time::Instant};

    fn png() -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 128, |x, y| {
            image::Rgb([x as u8, y as u8, 128])
        }));
        let mut bytes = Cursor::new(vec![]);
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn kitty_picker() -> Picker {
        let mut picker = picker_with_size(FontSize::new(8, 16));
        picker.set_protocol_type(ProtocolType::Kitty);
        picker
    }

    fn wait(images: &mut Images, image: &ImageRef, width: u16, ready: bool) {
        images.request(image, width);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            images.poll();
            let presentation = images.prepare(image, width);
            if ready && presentation.ready
                || !ready && presentation.caption.to_string().contains("failed")
            {
                return;
            }
            assert!(Instant::now() < deadline, "{}", presentation.caption);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn visible_images_load_once_and_resize_without_refetching() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("sample image.png"), png()).unwrap();
        let image = ImageRef {
            source: "sample%20image.png".into(),
            alt: "sample".into(),
        };
        let mut images = Images::default();
        images
            .enable(root.path().to_owned(), kitty_picker())
            .unwrap();
        images.prepare(&image, 8);
        assert!(images.assets.is_empty());
        wait(&mut images, &image, 8, true);
        let spec = Spec {
            width: 8,
            font: FontSize::new(8, 16),
        };
        let key = Key::new(&image.source, spec);
        let original = images.assets[&key].original.as_ref().unwrap().clone();
        let first_id = images.assets[&key].encoded.as_ref().unwrap().kitty_id;
        let requests = images.next_request;
        images.request(&image, 8);
        assert_eq!(images.next_request, requests);
        std::fs::remove_file(root.path().join("sample image.png")).unwrap();
        wait(&mut images, &image, 4, true);
        let smaller = Key::new(&image.source, Spec { width: 4, ..spec });
        assert!(Arc::ptr_eq(
            &original,
            images.assets[&smaller].original.as_ref().unwrap()
        ));
        assert_eq!(images.prepare(&image, 8).height, 8);
        assert_eq!(images.prepare(&image, 4).height, 4);
        images.request(&image, 8);
        assert_eq!(
            images.assets[&key].encoded.as_ref().unwrap().kitty_id,
            first_id
        );
        let mut zoomed = picker_with_size(FontSize::new(4, 8));
        zoomed.set_protocol_type(ProtocolType::Kitty);
        images.update_picker(zoomed);
        wait(&mut images, &image, 8, true);
        let zoomed_key = Key::new(
            &image.source,
            Spec {
                width: 8,
                font: FontSize::new(4, 8),
            },
        );
        assert!(Arc::ptr_eq(
            &original,
            images.assets[&zoomed_key].original.as_ref().unwrap()
        ));
    }

    #[test]
    fn kitty_slices_keep_original_rows_and_upload_only_once() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("sample.png"), png()).unwrap();
        let image = ImageRef {
            source: "sample.png".into(),
            alt: "sample".into(),
        };
        let mut images = Images::default();
        images
            .enable(root.path().to_owned(), kitty_picker())
            .unwrap();
        wait(&mut images, &image, 8, true);
        let mut clipped = Terminal::new(TestBackend::new(12, 6)).unwrap();
        clipped
            .draw(|frame| assert!(images.render(&image, 8, frame, Rect::new(2, 1, 8, 3), 2)))
            .unwrap();
        let first = clipped.backend().buffer()[(2, 1)].symbol().to_owned();
        assert!(first.contains("\u{1b}_G"));
        assert!(first.contains("U=1"));
        assert_eq!(clipped.backend().buffer()[(2, 0)].symbol(), " ");
        assert_eq!(clipped.backend().buffer()[(2, 4)].symbol(), " ");
        let mut full = Terminal::new(TestBackend::new(8, 8)).unwrap();
        full.draw(|frame| assert!(images.render(&image, 8, frame, Rect::new(0, 0, 8, 8), 0)))
            .unwrap();
        let placeholder = first.rfind('\u{10eeee}').unwrap();
        assert_eq!(
            &first[placeholder..],
            full.backend().buffer()[(0, 2)].symbol()
        );
        assert!(
            !full
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains("\u{1b}_G"))
        );
        let ids: Vec<_> = images
            .assets
            .values()
            .filter_map(|asset| asset.encoded.as_ref().and_then(|encoded| encoded.kitty_id))
            .collect();
        images.clear();
        let mut deletion = vec![];
        images.flush_retired(&mut deletion).unwrap();
        let deletion = String::from_utf8(deletion).unwrap();
        assert!(deletion.contains(&format!("a=d,d=I,i={}", ids[0])));
        let mut second = vec![];
        images.flush_retired(&mut second).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn http_images_share_decoded_data_between_sizes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let source = format!("http://{}/image.png", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = socket.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET "));
            let bytes = png();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            socket.write_all(&bytes).unwrap();
        });
        let image = ImageRef {
            source,
            alt: "remote image".into(),
        };
        let mut images = Images::default();
        images
            .enable(std::env::current_dir().unwrap(), kitty_picker())
            .unwrap();
        wait(&mut images, &image, 8, true);
        server.join().unwrap();
        wait(&mut images, &image, 4, true);
        assert!(images.prepare(&image, 4).ready);
    }

    #[test]
    fn bad_images_have_stable_fallbacks_and_limits_preserve_typed_errors() {
        let root = tempfile::tempdir().unwrap();
        let mut images = Images::default();
        images
            .enable(root.path().to_owned(), kitty_picker())
            .unwrap();
        let image = ImageRef {
            source: "missing.png".into(),
            alt: "missing chart".into(),
        };
        wait(&mut images, &image, 8, false);
        let requests = images.next_request;
        images.request(&image, 8);
        assert_eq!(images.next_request, requests);
        assert!(
            images
                .prepare(&image, 8)
                .caption
                .to_string()
                .contains("missing chart")
        );
        let file = File::create(root.path().join("large.png")).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        let epoch = AtomicU64::new(0);
        assert!(matches!(
            read_source(root.path(), "large.png", &mut None, &epoch, 0),
            Err(Error::Resource { .. })
        ));
        assert!(matches!(
            read_source(
                root.path(),
                "data:image/png;base64,AAAA",
                &mut None,
                &epoch,
                0
            ),
            Err(Error::Resource { .. })
        ));
        assert!(matches!(
            read_source(root.path(), "missing.png", &mut None, &epoch, 0),
            Err(Error::Io { .. })
        ));
        assert!(matches!(decode(vec![1, 2, 3]), Err(Error::Decode { .. })));
    }

    #[test]
    fn supported_codecs_decode_and_halfblocks_remain_available() {
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let image = DynamicImage::ImageRgb8(RgbImage::new(4, 4));
            let mut bytes = Cursor::new(vec![]);
            image.write_to(&mut bytes, format).unwrap();
            let decoded = decode(bytes.into_inner()).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (4, 4));
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("sample.png"), png()).unwrap();
        let mut picker = picker_with_size(FontSize::new(8, 16));
        picker.set_protocol_type(ProtocolType::Halfblocks);
        let image = ImageRef {
            source: "sample.png".into(),
            alt: String::new(),
        };
        let mut images = Images::default();
        images.enable(root.path().to_owned(), picker).unwrap();
        wait(&mut images, &image, 8, true);
        let mut terminal = Terminal::new(TestBackend::new(8, 4)).unwrap();
        terminal
            .draw(|frame| assert!(images.render(&image, 8, frame, frame.area(), 2)))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() != " ")
        );
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains('\u{1b}'))
        );
    }

    #[test]
    fn session_generations_reject_stale_completions() {
        let (requests, jobs) = mpsc::sync_channel(8);
        let (responses, results) = mpsc::sync_channel(2);
        let epoch = Arc::new(AtomicU64::new(0));
        let mut images = Images {
            picker: Some(kitty_picker()),
            worker: Some(Worker {
                requests,
                results,
                epoch: epoch.clone(),
            }),
            ..Default::default()
        };
        let reference = ImageRef {
            source: "chart.png".into(),
            alt: "chart".into(),
        };
        images.request(&reference, 8);
        let mut first = jobs.recv().unwrap();
        first.original = Some(Arc::new(decode(png()).unwrap()));
        let old = load(
            Path::new("/"),
            &first,
            &mut None,
            &epoch,
            &mut BTreeMap::new(),
        )
        .unwrap();
        images.clear();
        images.request(&reference, 8);
        let mut current = jobs.recv().unwrap();
        current.original = first.original.clone();
        responses
            .send(Completed {
                key: first.key,
                id: first.id,
                result: Ok(old),
            })
            .unwrap();
        images.poll();
        assert!(!images.prepare(&reference, 8).ready);
        assert!(
            matches!(images.assets[&current.key].state, State::Loading(id) if id == current.id)
        );
        let loaded = load(
            Path::new("/"),
            &current,
            &mut None,
            &epoch,
            &mut BTreeMap::new(),
        )
        .unwrap();
        responses
            .send(Completed {
                key: current.key,
                id: current.id,
                result: Ok(loaded),
            })
            .unwrap();
        images.poll();
        assert!(images.prepare(&reference, 8).ready);
    }

    #[test]
    fn a_full_viewport_keeps_pending_requests_and_cache_entries_bounded() {
        let (requests, jobs) = mpsc::sync_channel(8);
        let (_responses, results) = mpsc::sync_channel(2);
        let mut images = Images {
            picker: Some(kitty_picker()),
            worker: Some(Worker {
                requests,
                results,
                epoch: Arc::new(AtomicU64::new(0)),
            }),
            ..Default::default()
        };
        for _ in 0..3 {
            images.begin_frame();
            for index in 0..100 {
                images.request(
                    &ImageRef {
                        source: format!("chart-{index}.png"),
                        alt: String::new(),
                    },
                    8,
                );
            }
            assert_eq!(images.assets.len(), MAX_ENTRIES);
        }
        assert_eq!(jobs.try_iter().count(), 8);
        assert!(
            images
                .assets
                .values()
                .any(|asset| matches!(asset.state, State::Loading(1)))
        );
    }

    #[test]
    fn decoded_pixels_and_encoded_cache_have_separate_resource_limits() {
        let mut bytes = Cursor::new(vec![]);
        DynamicImage::ImageRgb8(RgbImage::new(2049, 2048))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        assert!(matches!(
            decode(bytes.into_inner()),
            Err(Error::Resource { .. })
        ));
        let mut job = Work {
            key: Key::new(
                "chart.png",
                Spec {
                    width: 8,
                    font: FontSize::new(8, 16),
                },
            ),
            id: 1,
            epoch: 0,
            original: Some(Arc::new(decode(png()).unwrap())),
            spec: Spec {
                width: 8,
                font: FontSize::new(8, 16),
            },
            picker: kitty_picker(),
        };
        let mut loaded = load(
            Path::new("/"),
            &job,
            &mut None,
            &AtomicU64::new(0),
            &mut BTreeMap::new(),
        )
        .unwrap();
        loaded.encoded.bytes = MAX_CACHE_BYTES + 1;
        let mut images = Images::default();
        assert!(!images.reserve(&loaded));
        job.spec.width = 4;
        let loaded = load(
            Path::new("/"),
            &job,
            &mut None,
            &AtomicU64::new(0),
            &mut BTreeMap::new(),
        )
        .unwrap();
        assert!(images.reserve(&loaded));
        job.spec.font = FontSize::new(u16::MAX, u16::MAX);
        assert!(matches!(
            load(
                Path::new("/"),
                &job,
                &mut None,
                &AtomicU64::new(0),
                &mut BTreeMap::new()
            ),
            Err(Error::Resource { .. })
        ));
    }
}
