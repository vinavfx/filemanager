use eframe::egui;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const THUMBNAIL_EDGE: u32 = 192;
const THUMBNAIL_VERSION: u32 = 2;
const MEMORY_CAPACITY: usize = 512;
pub const VIDEO_FRAMES: usize = 8;
pub const VIDEO_POSTER: usize = VIDEO_FRAMES / 2;
type ThumbnailKey = (PathBuf, usize);

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
        let image = if image.width() > edge || image.height() > edge {
            image.thumbnail(edge, edge)
        } else {
            image
        };
        let image = if matches!(
            image.color(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        ) {
            let float = image.to_rgba32f();
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
            image.to_rgba8()
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
    image: egui::ColorImage,
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
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .hash(&mut hasher);
    Ok(hasher.finish())
}

fn cached_thumbnail(
    path: &Path,
    directory: &Path,
    generation: &AtomicU64,
    requested_generation: u64,
    frame: usize,
) -> Result<Option<CachedImage>, String> {
    let active = || generation.load(Ordering::Relaxed) == requested_generation;
    if !active() {
        return Ok(None);
    }
    let key = fingerprint(path)?;
    let video = crate::files::video_file(path);
    let suffix = if video {
        format!("-video-v3-{VIDEO_FRAMES}-{frame}")
    } else {
        String::new()
    };
    let cached = directory.join(format!(
        "{key:016x}-{THUMBNAIL_EDGE}-v{THUMBNAIL_VERSION}{suffix}.webp"
    ));
    if let Ok(image) = load(&cached, THUMBNAIL_EDGE) {
        return Ok(active().then_some(CachedImage {
            key,
            image,
            warning: None,
        }));
    }
    if !active() {
        return Ok(None);
    }
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
        image,
        warning: saved.err().map(|error| error.to_string()),
    }))
}

pub struct Thumbnails {
    sender: mpsc::SyncSender<(u64, ThumbnailKey)>,
    receiver: mpsc::Receiver<(u64, ThumbnailKey, Result<CachedImage, String>)>,
    generation: Arc<AtomicU64>,
    pending: HashSet<ThumbnailKey>,
    cache: HashMap<ThumbnailKey, Result<egui::TextureHandle, String>>,
    fingerprints: HashMap<ThumbnailKey, u64>,
    order: VecDeque<ThumbnailKey>,
    completed: usize,
    total: usize,
    finished_until: f64,
    warning: Option<String>,
}

impl Thumbnails {
    pub fn new(context: egui::Context) -> Self {
        let (sender, requests) = mpsc::sync_channel::<(u64, ThumbnailKey)>(16);
        let requests = Arc::new(Mutex::new(requests));
        let (results, receiver) = mpsc::channel();
        let directory = cache_directory();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..2 {
            let requests = Arc::clone(&requests);
            let results = results.clone();
            let context = context.clone();
            let directory = directory.clone();
            let generation = Arc::clone(&generation);
            std::thread::spawn(move || loop {
                let request = match requests.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let (requested_generation, path) = match request {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let image = match cached_thumbnail(
                    &path.0,
                    &directory,
                    &generation,
                    requested_generation,
                    path.1,
                ) {
                    Ok(Some(image)) => Ok(image),
                    Ok(None) => {
                        context.request_repaint();
                        continue;
                    }
                    Err(error) => Err(error),
                };
                if results.send((requested_generation, path, image)).is_err() {
                    break;
                }
                context.request_repaint();
            });
        }
        Self {
            sender,
            receiver,
            generation,
            pending: HashSet::new(),
            cache: HashMap::new(),
            fingerprints: HashMap::new(),
            order: VecDeque::new(),
            completed: 0,
            total: 0,
            finished_until: 0.0,
            warning: None,
        }
    }

    pub fn poll(&mut self, context: &egui::Context) {
        while let Ok((generation, path, image)) = self.receiver.try_recv() {
            if generation != self.generation.load(Ordering::Relaxed) {
                continue;
            }
            self.pending.remove(&path);
            self.completed += 1;
            if self.cache.len() >= MEMORY_CAPACITY {
                if let Some(oldest) = self.order.pop_front() {
                    self.cache.remove(&oldest);
                    self.fingerprints.remove(&oldest);
                }
            }
            let key = image
                .as_ref()
                .map(|cached| cached.key)
                .ok()
                .or_else(|| fingerprint(&path.0).ok());
            if let Some(key) = key {
                self.fingerprints.insert(path.clone(), key);
            }
            let texture = image.map(|cached| {
                if let Some(warning) = cached.warning {
                    self.warning = Some(warning);
                }
                context.load_texture(
                    format!("{}#{}", path.0.display(), path.1),
                    cached.image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.order.push_back(path.clone());
            self.cache.insert(path, texture);
            if self.pending.is_empty() {
                self.finished_until = context.input(|input| input.time) + 0.8;
                context.request_repaint_after(Duration::from_millis(800));
            }
        }
    }

    pub fn get(&mut self, path: &Path, frame: usize) -> Option<&egui::TextureHandle> {
        let frame = if crate::files::video_file(path) {
            frame.min(VIDEO_FRAMES - 1)
        } else {
            0
        };
        let key = (path.to_path_buf(), frame);
        if self.cache.contains_key(&key)
            && self.fingerprints.get(&key).copied() != fingerprint(path).ok()
        {
            self.cache.remove(&key);
            self.fingerprints.remove(&key);
            self.order.retain(|cached| cached != &key);
        }
        if !self.cache.contains_key(&key)
            && !self.pending.contains(&key)
            && self
                .sender
                .try_send((self.generation.load(Ordering::Relaxed), key.clone()))
                .is_ok()
        {
            if self.pending.is_empty() {
                self.completed = 0;
                self.total = 0;
            }
            self.total += 1;
            self.pending.insert(key.clone());
        }
        if self.cache.contains_key(&key) {
            self.order.retain(|cached| cached != &key);
            self.order.push_back(key.clone());
        }
        let fallback = (
            path.to_path_buf(),
            if crate::files::video_file(path) {
                VIDEO_POSTER
            } else {
                0
            },
        );
        self.cache
            .get(&key)
            .and_then(|result| result.as_ref().ok())
            .or_else(|| {
                self.cache
                    .get(&fallback)
                    .and_then(|result| result.as_ref().ok())
            })
    }

    pub fn get_entry(
        &mut self,
        entry: &crate::files::Entry,
        frame: usize,
    ) -> Option<&egui::TextureHandle> {
        let poster = if entry.animated() { VIDEO_POSTER } else { 0 };
        let (path, source_frame) = entry.thumbnail(poster);
        self.get(path, source_frame);
        let (path, source_frame) = entry.thumbnail(frame);
        self.get(path, source_frame);
        let displayed = self.displayed_frame(entry, frame)?;
        let (path, source_frame) = entry.thumbnail(displayed);
        self.cache
            .get(&(path.to_path_buf(), source_frame))?
            .as_ref()
            .ok()
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

    pub fn entry_aspect_ratio(&self, entry: &crate::files::Entry) -> f32 {
        let (path, _) = entry.thumbnail(VIDEO_POSTER);
        self.aspect_ratio(path)
    }

    pub fn entry_error(&self, entry: &crate::files::Entry) -> Option<&str> {
        (0..VIDEO_FRAMES).find_map(|frame| self.error(entry.thumbnail(frame).0))
    }

    pub fn aspect_ratio(&self, path: &Path) -> f32 {
        let frame = if crate::files::video_file(path) {
            VIDEO_POSTER
        } else {
            0
        };
        self.cache
            .get(&(path.to_path_buf(), frame))
            .and_then(|result| result.as_ref().ok())
            .map_or(1.0, |texture| {
                let size = texture.size_vec2();
                size.x / size.y
            })
    }

    pub fn progress(&self, context: &egui::Context) -> Option<(usize, usize)> {
        if self.total > 0
            && (!self.pending.is_empty() || context.input(|input| input.time) < self.finished_until)
        {
            Some((self.completed, self.total))
        } else {
            None
        }
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn error(&self, path: &Path) -> Option<&str> {
        (0..VIDEO_FRAMES).find_map(|frame| {
            self.cache
                .get(&(path.to_path_buf(), frame))
                .and_then(|result| result.as_ref().err())
                .map(String::as_str)
        })
    }

    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending.clear();
        self.cache.retain(|_, texture| texture.is_ok());
        self.fingerprints
            .retain(|path, _| self.cache.contains_key(path));
        self.order.retain(|path| self.cache.contains_key(path));
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
