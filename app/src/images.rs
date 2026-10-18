use eframe::egui;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const THUMBNAIL_EDGE: u32 = 192;
const THUMBNAIL_VERSION: u32 = 3;
const MEMORY_BUDGET_BYTES: usize = 1024 * 1024 * 1024;
const MAX_PENDING: usize = 48;
const MAX_UPLOADS_PER_FRAME: usize = 8;
pub const VIDEO_FRAMES: usize = 8;
pub const VIDEO_POSTER: usize = VIDEO_FRAMES / 2;
type ThumbnailKey = (PathBuf, usize);
type ThumbnailRequest = (u64, ThumbnailKey, Option<u64>);
type ThumbnailResult = (u64, ThumbnailKey, Option<u64>, Result<CachedImage, String>);

fn media_output(
    command: &mut Command,
    generation: &AtomicU64,
    requested_generation: u64,
) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot start media decoder: {error}"))?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stdout) = stdout {
            stdout.take(1024 * 1024).read_to_end(&mut bytes)?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let started = Instant::now();
    let status = loop {
        if generation.load(Ordering::Relaxed) != requested_generation
            || started.elapsed() > Duration::from_secs(15)
        {
            let _ = child.kill();
            let _ = child.wait();
            break Err("Media decoding cancelled or timed out".to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error.to_string());
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "Media output reader failed".to_owned())?
        .map_err(|error| error.to_string())?;
    if !status?.success() || bytes.is_empty() {
        return Err("Cannot decode video; the format or codec may be unsupported".to_owned());
    }
    Ok(bytes)
}

fn video_frame(
    path: &Path,
    frame: usize,
    generation: &AtomicU64,
    requested_generation: u64,
) -> Result<egui::ColorImage, String> {
    let mut seconds = 0.0;
    if frame > 0 {
        let bytes = media_output(
            Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration:stream=duration",
                    "-select_streams",
                    "v:0",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(path),
            generation,
            requested_generation,
        )?;
        let duration = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| line.trim().parse::<f64>().ok())
            .find(|duration| duration.is_finite() && *duration > 0.0)
            .ok_or_else(|| "Video duration unavailable".to_owned())?;
        seconds = if frame == VIDEO_POSTER {
            duration * 0.5
        } else {
            (duration * 0.98).min((duration - 0.1).max(0.0)) * frame as f64
                / (VIDEO_FRAMES - 1) as f64
        };
    }
    let bytes = media_output(
        Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-threads", "1", "-ss"])
            .arg(format!("{seconds:.6}"))
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-an",
                "-sn",
                "-vf",
                "scale=w='max(1,round(192*min(1,dar)))':h='max(1,round(192/max(1,dar)))',setsar=1",
                "-threads",
                "1",
                "-f",
                "image2pipe",
                "-c:v",
                "png",
                "pipe:1",
            ]),
        generation,
        requested_generation,
    )?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| error.to_string())?
        .to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

pub fn load(path: &Path, edge: u32) -> Result<egui::ColorImage, String> {
    let result = (|| -> Result<egui::ColorImage, Box<dyn std::error::Error>> {
        let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode()?;
        let floating_point = matches!(
            image.color(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        );
        let image = if image.width() > edge || image.height() > edge {
            if floating_point {
                image.resize(edge, edge, image::imageops::FilterType::Triangle)
            } else {
                image.thumbnail(edge, edge)
            }
        } else {
            image
        };
        if let Some(image) = image.as_rgb8() {
            return Ok(egui::ColorImage::from_rgb(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ));
        }
        let image = if floating_point {
            let float = image.into_rgba32f();
            image::RgbaImage::from_fn(float.width(), float.height(), |x, y| {
                let pixel = float.get_pixel(x, y);
                let mut rgba = [0; 4];
                for channel in 0..3 {
                    let linear = pixel[channel].clamp(0.0, 1.0);
                    let srgb = if linear <= 0.0031308 {
                        linear * 12.92
                    } else {
                        1.055 * linear.powf(1.0 / 2.4) - 0.055
                    };
                    rgba[channel] = (srgb * 255.0).round() as u8;
                }
                rgba[3] = (pixel[3].clamp(0.0, 1.0) * 255.0).round() as u8;
                image::Rgba(rgba)
            })
        } else {
            image.into_rgba8()
        };
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ))
    })();
    result.map_err(|error| error.to_string())
}

struct CachedImage {
    key: u64,
    image: Option<egui::ColorImage>,
    warning: Option<String>,
}

fn cache_directory() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".cache")
        });
    base.join("still-files/thumbnails")
}

pub fn fingerprint(path: &Path) -> Result<u64, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    Ok(metadata_fingerprint(path, &metadata))
}

fn metadata_fingerprint(path: &Path, metadata: &std::fs::Metadata) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .hash(&mut hasher);
    hasher.finish()
}

fn thumbnail_path(directory: &Path, key: u64, video: bool, frame: usize) -> PathBuf {
    let suffix = if video {
        format!("-video-v3-{VIDEO_FRAMES}-{frame}")
    } else {
        String::new()
    };
    directory.join(format!(
        "{key:016x}-{THUMBNAIL_EDGE}-v{THUMBNAIL_VERSION}{suffix}.webp"
    ))
}

pub fn copy_cached_thumbnails(from: &Path, to: &Path) {
    let video = crate::files::video_file(from);
    if !video && !crate::files::image_file(from) {
        return;
    }
    let (Ok(source), Ok(destination)) = (fingerprint(from), fingerprint(to)) else {
        return;
    };
    let directory = cache_directory();
    for frame in 0..if video { VIDEO_FRAMES } else { 1 } {
        let source = thumbnail_path(&directory, source, video, frame);
        let destination = thumbnail_path(&directory, destination, video, frame);
        let _ = std::fs::hard_link(source, destination);
    }
}

pub fn rename_cached_thumbnails(from: &Path, to: &Path) {
    let video = crate::files::video_file(from);
    if from == to
        || video != crate::files::video_file(to)
        || crate::files::image_file(from) != crate::files::image_file(to)
        || (!video && !crate::files::image_file(from))
    {
        return;
    }
    let Ok(metadata) = to.metadata() else {
        return;
    };
    let source = metadata_fingerprint(from, &metadata);
    let destination = metadata_fingerprint(to, &metadata);
    let directory = cache_directory();
    for frame in 0..if video { VIDEO_FRAMES } else { 1 } {
        let source = thumbnail_path(&directory, source, video, frame);
        let destination = thumbnail_path(&directory, destination, video, frame);
        let _ = std::fs::rename(source, destination);
    }
}

fn cached_thumbnail(
    path: &Path,
    directory: &Path,
    generation: &AtomicU64,
    requested_generation: u64,
    frame: usize,
    key: u64,
) -> Result<Option<CachedImage>, String> {
    let active = || generation.load(Ordering::Relaxed) == requested_generation;
    if !active() {
        return Ok(None);
    }
    let video = crate::files::video_file(path);
    let cached = thumbnail_path(directory, key, video, frame);
    let image = if video {
        video_frame(path, frame, generation, requested_generation)?
    } else {
        load(path, THUMBNAIL_EDGE)?
    };
    if !active() {
        return Ok(None);
    }
    let saved = (|| -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(directory)?;
        let temporary =
            cached.with_extension(format!("{}.{requested_generation}.tmp", std::process::id()));
        let bytes: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(egui::Color32::to_srgba_unmultiplied)
            .collect();
        image::save_buffer_with_format(
            &temporary,
            &bytes,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::WebP,
        )?;
        if active() {
            std::fs::rename(&temporary, &cached)?;
        } else {
            std::fs::remove_file(&temporary)?;
        }
        Ok(())
    })();
    Ok(active().then_some(CachedImage {
        key,
        image: Some(image),
        warning: saved.err().map(|error| error.to_string()),
    }))
}

pub struct Thumbnails {
    sender: mpsc::SyncSender<ThumbnailRequest>,
    receiver: mpsc::Receiver<ThumbnailResult>,
    generation: Arc<AtomicU64>,
    pending: HashMap<ThumbnailKey, bool>,
    cache: HashMap<ThumbnailKey, Result<egui::TextureHandle, String>>,
    cache_bytes: usize,
    revision: u64,
    fingerprints: HashMap<ThumbnailKey, (Option<u64>, Instant)>,
    last_used: HashMap<ThumbnailKey, Instant>,
    frame_used: HashSet<ThumbnailKey>,
    completed: usize,
    total: usize,
    finished_until: f64,
    warning: Option<String>,
}

impl Thumbnails {
    pub fn new(context: egui::Context) -> Self {
        let (sender, requests) = mpsc::sync_channel::<ThumbnailRequest>(MAX_PENDING);
        let requests = Arc::new(Mutex::new(requests));
        let (results, receiver) = mpsc::sync_channel(MAX_PENDING);
        let (decode, decoding) = mpsc::sync_channel::<(u64, ThumbnailKey, u64)>(MAX_PENDING);
        let decoding = Arc::new(Mutex::new(decoding));
        let directory = cache_directory();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..2 {
            let requests = Arc::clone(&requests);
            let results = results.clone();
            let context = context.clone();
            let directory = directory.clone();
            let generation = Arc::clone(&generation);
            let decode = decode.clone();
            std::thread::spawn(move || loop {
                let request = match requests.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let (requested_generation, path, previous) = match request {
                    Ok(request) => request,
                    Err(_) => break,
                };
                if generation.load(Ordering::Relaxed) != requested_generation {
                    continue;
                }
                let image = fingerprint(&path.0).map(|key| {
                    if previous == Some(key) {
                        return Some(CachedImage {
                            key,
                            image: None,
                            warning: None,
                        });
                    }
                    let cached =
                        thumbnail_path(&directory, key, crate::files::video_file(&path.0), path.1);
                    match load(&cached, THUMBNAIL_EDGE) {
                        Ok(image) => Some(CachedImage {
                            key,
                            image: Some(image),
                            warning: None,
                        }),
                        Err(_) => {
                            let _ = decode.send((requested_generation, path.clone(), key));
                            None
                        }
                    }
                });
                let image = match image {
                    Ok(Some(image)) => Ok(image),
                    Ok(None) => continue,
                    Err(error) => Err(error),
                };
                let key = image.as_ref().ok().map(|cached| cached.key);
                if results
                    .send((requested_generation, path, key, image))
                    .is_err()
                {
                    break;
                }
                context.request_repaint();
            });
        }
        let workers = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .saturating_sub(1)
            .clamp(1, 4);
        for _ in 0..workers {
            let requests = Arc::clone(&decoding);
            let results = results.clone();
            let context = context.clone();
            let directory = directory.clone();
            let generation = Arc::clone(&generation);
            std::thread::spawn(move || loop {
                let request = match requests.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let (requested_generation, path, key) = match request {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let image = match cached_thumbnail(
                    &path.0,
                    &directory,
                    &generation,
                    requested_generation,
                    path.1,
                    key,
                ) {
                    Ok(Some(image)) => Ok(image),
                    Ok(None) => {
                        context.request_repaint();
                        continue;
                    }
                    Err(error) => Err(error),
                };
                if results
                    .send((requested_generation, path, Some(key), image))
                    .is_err()
                {
                    break;
                }
                context.request_repaint();
            });
        }
        Self {
            sender,
            receiver,
            generation,
            pending: HashMap::new(),
            cache: HashMap::new(),
            cache_bytes: 0,
            revision: 0,
            fingerprints: HashMap::new(),
            last_used: HashMap::new(),
            frame_used: HashSet::new(),
            completed: 0,
            total: 0,
            finished_until: 0.0,
            warning: None,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_loading(&self) -> bool {
        self.pending.values().any(|loading| *loading)
    }

    pub fn poll(&mut self, context: &egui::Context) {
        self.frame_used.clear();
        let started = Instant::now();
        let mut uploads = 0;
        while uploads < MAX_UPLOADS_PER_FRAME && started.elapsed() < Duration::from_millis(3) {
            let Ok((generation, path, key, image)) = self.receiver.try_recv() else {
                break;
            };
            if generation != self.generation.load(Ordering::Relaxed) {
                continue;
            }
            let loading = self.pending.remove(&path).unwrap_or(false);
            if loading {
                self.completed += 1;
            }
            self.fingerprints
                .insert(path.clone(), (key, Instant::now()));
            if loading && !self.is_loading() {
                self.finished_until = context.input(|input| input.time) + 0.8;
                context.request_repaint_after(Duration::from_millis(800));
            }
            let image = match image {
                Ok(CachedImage { image: None, .. }) => continue,
                Ok(cached) => cached
                    .image
                    .map(|image| (image, cached.warning))
                    .ok_or_else(|| "Thumbnail image unavailable".to_owned()),
                Err(error) => Err(error),
            };
            let texture = image.map(|(image, warning)| {
                if let Some(warning) = warning {
                    self.warning = Some(warning);
                }
                context.load_texture(
                    format!("{}#{}", path.0.display(), path.1),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            if let Ok(texture) = &texture {
                uploads += 1;
                context.animate_bool_with_time(
                    egui::Id::new(("loaded-texture", texture.id())),
                    self.cache.get(&path).is_some_and(|image| image.is_ok()),
                    0.12,
                );
            }
            let bytes = texture.as_ref().map_or(0, egui::TextureHandle::byte_size);
            while self.cache_bytes + bytes > MEMORY_BUDGET_BYTES {
                let Some(oldest) = self
                    .last_used
                    .iter()
                    .min_by_key(|(_, accessed)| **accessed)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                self.remove_cached_texture(&oldest);
                self.fingerprints.remove(&oldest);
            }
            self.remove_cached_texture(&path);
            self.last_used.insert(path.clone(), Instant::now());
            self.cache.insert(path, texture);
            self.revision = self.revision.wrapping_add(1);
            self.cache_bytes += bytes;
        }
        if uploads == MAX_UPLOADS_PER_FRAME || started.elapsed() >= Duration::from_millis(3) {
            context.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn remove_cached_texture(&mut self, key: &ThumbnailKey) {
        self.last_used.remove(key);
        if let Some(texture) = self.cache.remove(key) {
            self.revision = self.revision.wrapping_add(1);
            if let Ok(texture) = texture {
                self.cache_bytes -= texture.byte_size();
            }
        }
    }

    pub fn reuse_copies(&mut self, copies: &[(PathBuf, PathBuf)]) {
        self.reuse_paths(copies, false);
    }

    pub fn reuse_renames(&mut self, renames: &[(PathBuf, PathBuf)]) {
        self.reuse_paths(renames, true);
    }

    fn reuse_paths(&mut self, paths: &[(PathBuf, PathBuf)], renamed: bool) {
        if paths.is_empty() {
            return;
        }
        let reused: Vec<_> = self
            .cache
            .iter()
            .filter_map(|((path, frame), image)| {
                let destination = paths.iter().find_map(|(from, to)| {
                    let suffix = path.strip_prefix(from).ok()?;
                    Some(if suffix.as_os_str().is_empty() {
                        to.clone()
                    } else {
                        to.join(suffix)
                    })
                })?;
                if path == &destination
                    || crate::files::video_file(path) != crate::files::video_file(&destination)
                    || crate::files::image_file(path) != crate::files::image_file(&destination)
                {
                    return None;
                }
                let texture = image.as_ref().ok()?;
                let stored = self.fingerprints.get(&(path.clone(), *frame))?.0?;
                let metadata = destination.metadata().ok()?;
                let source = if renamed {
                    Some(metadata_fingerprint(path, &metadata))
                } else {
                    fingerprint(path).ok()
                };
                if source != Some(stored) {
                    return None;
                }
                let key = metadata_fingerprint(&destination, &metadata);
                Some((
                    (path.clone(), *frame),
                    (destination, *frame),
                    stored,
                    key,
                    texture.clone(),
                ))
            })
            .collect();
        for (source, path, stored, key, texture) in reused {
            if renamed {
                let directory = cache_directory();
                let video = crate::files::video_file(&source.0);
                let _ = std::fs::rename(
                    thumbnail_path(&directory, stored, video, source.1),
                    thumbnail_path(&directory, key, video, path.1),
                );
                self.remove_cached_texture(&source);
                self.fingerprints.remove(&source);
            }
            self.remove_cached_texture(&path);
            let bytes = texture.byte_size();
            if self.cache_bytes + bytes > MEMORY_BUDGET_BYTES {
                continue;
            }
            self.cache_bytes += bytes;
            self.fingerprints
                .insert(path.clone(), (Some(key), Instant::now()));
            self.last_used.insert(path.clone(), Instant::now());
            self.cache.insert(path, Ok(texture));
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn get(&mut self, path: &Path, frame: usize) -> Option<&egui::TextureHandle> {
        let frame = if crate::files::video_file(path) {
            frame.min(VIDEO_FRAMES - 1)
        } else {
            0
        };
        let key = (path.to_path_buf(), frame);
        let outdated = self
            .fingerprints
            .get(&key)
            .is_none_or(|(_, checked)| checked.elapsed() >= Duration::from_secs(1));
        if (outdated || !self.cache.contains_key(&key))
            && !self.pending.contains_key(&key)
            && self.pending.len() < MAX_PENDING
            && self
                .sender
                .try_send((
                    self.generation.load(Ordering::Relaxed),
                    key.clone(),
                    self.cache
                        .get(&key)
                        .and_then(|_| self.fingerprints.get(&key)?.0),
                ))
                .is_ok()
        {
            let loading = !self.cache.contains_key(&key);
            if loading {
                if !self.is_loading() {
                    self.completed = 0;
                    self.total = 0;
                }
                self.total += 1;
            }
            self.pending.insert(key.clone(), loading);
        }
        let fallback = (
            path.to_path_buf(),
            if crate::files::video_file(path) {
                VIDEO_POSTER
            } else {
                0
            },
        );
        let displayed = if self.cache.get(&key).is_some_and(Result::is_ok) {
            key
        } else {
            fallback
        };
        if self.cache.contains_key(&displayed) {
            self.last_used.insert(displayed.clone(), Instant::now());
            self.frame_used.insert(displayed.clone());
        }
        self.cache
            .get(&displayed)
            .and_then(|result| result.as_ref().ok())
    }

    pub fn get_entry(
        &mut self,
        entry: &crate::files::Entry,
        frame: usize,
    ) -> Option<&egui::TextureHandle> {
        let poster = if entry.animated() { VIDEO_POSTER } else { 0 };
        let (path, source_frame) = entry.thumbnail(poster);
        self.get(path, source_frame);
        if frame != poster {
            let (path, source_frame) = entry.thumbnail(frame);
            self.get(path, source_frame);
        }
        let displayed = self.displayed_frame(entry, frame)?;
        let (path, source_frame) = entry.thumbnail(displayed);
        self.cache
            .get(&(path.to_path_buf(), source_frame))?
            .as_ref()
            .ok()
    }

    pub fn textures(&self) -> impl Iterator<Item = &egui::TextureHandle> {
        self.frame_used
            .iter()
            .filter_map(|key| self.cache.get(key)?.as_ref().ok())
    }

    pub fn displayed_frame(&self, entry: &crate::files::Entry, frame: usize) -> Option<usize> {
        let poster = if entry.animated() { VIDEO_POSTER } else { 0 };
        [frame, poster].into_iter().find(|frame| {
            let (path, source_frame) = entry.thumbnail(*frame);
            self.cache
                .get(&(path.to_path_buf(), source_frame))
                .is_some_and(|image| image.is_ok())
        })
    }

    pub fn entry_ready(&self, entry: &crate::files::Entry) -> bool {
        let poster = if entry.animated() { VIDEO_POSTER } else { 0 };
        let (path, frame) = entry.thumbnail(poster);
        self.cache.contains_key(&(path.to_path_buf(), frame))
    }

    pub fn entry_aspect_ratio(&self, entry: &crate::files::Entry) -> Option<f32> {
        let (path, _) = entry.thumbnail(VIDEO_POSTER);
        self.aspect_ratio(path)
    }

    fn aspect_ratio(&self, path: &Path) -> Option<f32> {
        let frame = if crate::files::video_file(path) {
            VIDEO_POSTER
        } else {
            0
        };
        self.cache
            .get(&(path.to_path_buf(), frame))
            .and_then(|result| result.as_ref().ok())
            .map(|texture| {
                let size = texture.size_vec2();
                size.x / size.y
            })
    }

    pub fn progress(&self, context: &egui::Context) -> Option<(usize, usize)> {
        if self.total > 0
            && (self.is_loading() || context.input(|input| input.time) < self.finished_until)
        {
            Some((self.completed, self.total))
        } else {
            None
        }
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.revision = self.revision.wrapping_add(1);
        self.pending.clear();
        self.frame_used.clear();
        self.cache.retain(|_, texture| texture.is_ok());
        self.fingerprints
            .retain(|path, _| self.cache.contains_key(path));
        self.last_used
            .retain(|path, _| self.cache.contains_key(path));
        self.completed = 0;
        self.total = 0;
        self.finished_until = 0.0;
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        self.cancel();
    }
}
