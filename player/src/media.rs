use crate::playback::Rate;
use crate::thumbnail_cache::{cache_directory, fingerprint, thumbnail_path};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

pub const CACHE_LIMIT: usize = 24 * 1024 * 1024 * 1024;
const PRELOAD_FRAMES: usize = 4;

pub enum Pixels {
    Bytes(Vec<u8>),
    Float(Vec<f32>),
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    pub gamma: f32,
    pub smooth_preview: bool,
}

impl Frame {
    pub fn bytes(&self) -> usize {
        match &self.pixels {
            Pixels::Bytes(pixels) => pixels.len(),
            Pixels::Float(pixels) => pixels.len() * size_of::<f32>(),
        }
    }
}

#[derive(Clone)]
pub struct Media {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub count: usize,
    pub numbers: Vec<i64>,
    files: Vec<PathBuf>,
}

impl Media {
    pub fn is_still(&self) -> bool {
        self.files.len() == 1
            && image_extension(&self.path)
            && !self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(['#', '%']))
    }

    pub fn number(&self, index: usize) -> i64 {
        self.numbers.get(index).copied().unwrap_or(index as i64)
    }

    pub fn index(&self, number: i64) -> usize {
        if self.numbers.is_empty() {
            number.clamp(0, self.count.saturating_sub(1) as i64) as usize
        } else {
            match self.numbers.binary_search(&number) {
                Ok(index) => index,
                Err(index) => index.min(self.count.saturating_sub(1)),
            }
        }
    }
}

#[derive(Default)]
pub struct Cache {
    pub generation: u64,
    pub frames: BTreeMap<usize, Arc<Frame>>,
    pub bytes: usize,
    order: VecDeque<usize>,
}

impl Cache {
    fn insert(&mut self, index: usize, frame: Arc<Frame>) {
        if let Some(old) = self.frames.remove(&index) {
            self.bytes -= old.bytes();
            self.order.retain(|&value| value != index);
        }
        while self.bytes.saturating_add(frame.bytes()) > CACHE_LIMIT && !self.order.is_empty() {
            if let Some(index) = self.order.pop_front()
                && let Some(old) = self.frames.remove(&index)
            {
                self.bytes -= old.bytes();
            }
        }
        self.bytes += frame.bytes();
        self.frames.insert(index, frame);
        self.order.push_back(index);
    }
}

#[derive(Default)]
struct ImageCache {
    frames: VecDeque<(u64, Arc<Frame>)>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, key: u64) -> Option<Arc<Frame>> {
        let index = self.frames.iter().position(|entry| entry.0 == key)?;
        let entry = self.frames.remove(index)?;
        let frame = entry.1.clone();
        self.frames.push_back(entry);
        Some(frame)
    }

    fn remove(&mut self, key: u64) {
        if let Some(index) = self.frames.iter().position(|entry| entry.0 == key)
            && let Some((_, frame)) = self.frames.remove(index)
        {
            self.bytes -= frame.bytes();
        }
    }

    fn evict(&mut self, cache: &mut Cache) {
        if let Some((_, oldest)) = self.frames.pop_front() {
            self.bytes -= oldest.bytes();
            cache.frames.retain(|_, frame| {
                if Arc::ptr_eq(frame, &oldest) {
                    cache.bytes -= frame.bytes();
                    false
                } else {
                    true
                }
            });
            cache.order.retain(|index| cache.frames.contains_key(index));
        }
    }

    fn insert(&mut self, key: u64, frame: Arc<Frame>, cache: &mut Cache) {
        self.remove(key);
        if frame.bytes() > CACHE_LIMIT {
            return;
        }
        while self.bytes + frame.bytes() > CACHE_LIMIT && !self.frames.is_empty() {
            self.evict(cache);
        }
        self.bytes += frame.bytes();
        self.frames.push_back((key, frame));
    }
}

fn image_key(path: &Path) -> Option<u64> {
    let metadata = path.metadata().ok()?;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata.modified().ok()?.hash(&mut hasher);
    Some(hasher.finish())
}

pub enum Request {
    Open(u64, PathBuf),
    Seek(u64, usize),
    ReloadFrame(u64, usize),
    Close(u64),
}

pub enum Event {
    Preview(u64, Arc<Frame>, (u32, u32)),
    Loaded(u64, Media),
    Error(u64, String),
}

pub struct Loader {
    pub cache: Arc<Mutex<Cache>>,
    pub events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Request>,
}

impl Loader {
    pub fn new(context: eframe::egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (events, event_receiver) = mpsc::channel();
        let cache = Arc::new(Mutex::new(Cache::default()));
        let worker_cache = cache.clone();
        thread::spawn(move || worker(receiver, events, worker_cache, context));
        Self {
            cache,
            events: event_receiver,
            sender,
        }
    }

    pub fn send(&self, request: Request) {
        let _ = self.sender.send(request);
    }

    pub fn frame(&self, generation: u64, index: usize) -> Option<Arc<Frame>> {
        let cache = self.cache.lock().ok()?;
        (cache.generation == generation)
            .then(|| cache.frames.get(&index).cloned())
            .flatten()
    }
}

fn worker(
    receiver: mpsc::Receiver<Request>,
    events: mpsc::Sender<Event>,
    cache: Arc<Mutex<Cache>>,
    context: eframe::egui::Context,
) {
    let mut generation = 0;
    let mut decoder: Option<Decoder> = None;
    let mut target = 0;
    let mut preload = 0;
    let mut failed = HashSet::new();
    let mut images = ImageCache::default();
    loop {
        let request = match receiver.recv_timeout(Duration::from_millis(2)) {
            Ok(request) => Some(request),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut requests: Vec<_> = request.into_iter().collect();
        requests.extend(receiver.try_iter());
        for request in requests {
            match request {
                Request::Open(id, path) => {
                    generation = id;
                    decoder = None;
                    failed.clear();
                    target = 0;
                    preload = 0;
                    if let Ok(mut cache) = cache.lock() {
                        *cache = Cache {
                            generation,
                            ..Cache::default()
                        };
                    }
                    let retained = std::path::absolute(&path)
                        .ok()
                        .and_then(|path| image_key(&path))
                        .is_some_and(|key| images.frames.iter().any(|entry| entry.0 == key));
                    if !retained && let Some((frame, dimensions)) = cached_preview(&path) {
                        let _ =
                            events.send(Event::Preview(generation, Arc::new(frame), dimensions));
                        context.request_repaint();
                    }
                    match inspect(&path) {
                        Ok(media) => {
                            if let Some(key) = media.files.first().and_then(|path| image_key(path))
                                && let Some(frame) = images.get(key)
                                && let Ok(mut cache) = cache.lock()
                            {
                                cache.insert(0, frame);
                            }
                            let _ = events.send(Event::Loaded(generation, media.clone()));
                            decoder = Some(Decoder { media, pipe: None });
                        }
                        Err(error) => {
                            let _ = events.send(Event::Error(generation, error));
                        }
                    }
                    context.request_repaint();
                }
                Request::Seek(id, frame) if id == generation => {
                    if let Some(key) = decoder
                        .as_ref()
                        .and_then(|decoder| decoder.media.files.get(frame))
                        .and_then(|path| image_key(path))
                    {
                        images.get(key);
                    }
                    target = frame;
                    preload = frame;
                }
                Request::ReloadFrame(id, frame) if id == generation => {
                    if let Ok(mut cache) = cache.lock()
                        && let Some(old) = cache.frames.remove(&frame)
                    {
                        cache.bytes -= old.bytes();
                        cache.order.retain(|&value| value != frame);
                    }
                    failed.remove(&frame);
                    if let Some(decoder) = &mut decoder {
                        if let Some(key) = decoder
                            .media
                            .files
                            .get(frame)
                            .and_then(|path| image_key(path))
                        {
                            images.remove(key);
                        }
                        decoder.pipe = None;
                    }
                    target = frame;
                    preload = frame;
                }
                Request::Close(id) => {
                    generation = id;
                    decoder = None;
                    if let Ok(mut cache) = cache.lock() {
                        *cache = Cache {
                            generation,
                            ..Cache::default()
                        };
                    }
                }
                _ => {}
            }
        }
        let Some(decoder) = &mut decoder else {
            continue;
        };
        let count = decoder.media.count;
        let indices = {
            let Ok(cache) = cache.lock() else { break };
            let mut indices = Vec::new();
            if target < count && !cache.frames.contains_key(&target) && !failed.contains(&target) {
                indices.push(target);
            }
            let parallel = if decoder.media.files.is_empty() {
                1
            } else {
                PRELOAD_FRAMES
            };
            let estimate = decoder.media.width as usize * decoder.media.height as usize * 16;
            let scans = if cache.frames.len() + failed.len() >= count {
                0
            } else {
                count.min(1024)
            };
            for _ in 0..scans {
                if indices.len() >= parallel
                    || cache
                        .bytes
                        .saturating_add(estimate.saturating_mul(indices.len() + 1))
                        > CACHE_LIMIT
                {
                    break;
                }
                let candidate = preload % count;
                preload = (preload + 1) % count;
                if !cache.frames.contains_key(&candidate)
                    && !failed.contains(&candidate)
                    && !indices.contains(&candidate)
                {
                    indices.push(candidate);
                }
            }
            indices
        };
        if indices.is_empty() {
            thread::sleep(Duration::from_millis(12));
            continue;
        }
        thread::scope(|scope| {
            let (results, receiver) = mpsc::channel();
            for index in indices {
                let path = decoder.media.files.get(index).cloned();
                let key = path.as_deref().and_then(image_key);
                if let Some(frame) = key.and_then(|key| images.get(key)) {
                    let _ = results.send((index, key, Ok(frame)));
                } else if let Some(path) = path {
                    let results = results.clone();
                    scope.spawn(move || {
                        let frame = decode_image(&path).map(Arc::new);
                        let _ = results.send((index, key, frame));
                    });
                } else {
                    let frame = decoder.read(index).map(Arc::new);
                    let _ = results.send((index, key, frame));
                }
            }
            drop(results);
            let mut frames: Vec<_> = receiver.into_iter().collect();
            frames.sort_unstable_by_key(|(index, _, _)| *index);
            let Ok(mut cache) = cache.lock() else { return };
            for (index, key, frame) in frames {
                match frame {
                    Ok(frame) => {
                        if let Some(key) = key
                            && decoder
                                .media
                                .files
                                .get(index)
                                .and_then(|path| image_key(path))
                                == Some(key)
                        {
                            images.insert(key, frame.clone(), &mut cache);
                        } else {
                            while images.bytes + cache.bytes + frame.bytes() > CACHE_LIMIT
                                && !images.frames.is_empty()
                            {
                                images.evict(&mut cache);
                            }
                        }
                        cache.insert(index, frame);
                    }
                    Err(error) => {
                        failed.insert(index);
                        let _ = events.send(Event::Error(
                            generation,
                            format!("Frame {}: {error}", decoder.media.number(index)),
                        ));
                    }
                }
            }
            drop(cache);
            context.request_repaint();
        });
    }
}

fn cached_preview(path: &Path) -> Option<(Frame, (u32, u32))> {
    let path = std::path::absolute(path).ok()?;
    let path = if path.is_file() {
        path
    } else {
        sequence(&path).ok()?.first()?.1.clone()
    };
    if !image_extension(&path) {
        return None;
    }
    let cached = thumbnail_path(&cache_directory(), fingerprint(&path).ok()?, false);
    let image = image::open(cached).ok()?.into_rgba8();
    let dimensions = image::ImageReader::open(&path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_dimensions().ok())
        .unwrap_or_else(|| image.dimensions());
    Some((
        Frame {
            width: image.width(),
            height: image.height(),
            pixels: Pixels::Bytes(image.into_raw()),
            gamma: 1.0,
            smooth_preview: true,
        },
        dimensions,
    ))
}

fn image_extension(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    image::ImageFormat::from_extension(&extension).is_some()
        || matches!(
            extension.as_str(),
            "dpx" | "cin" | "sgi" | "rgb" | "rgba" | "iff" | "pic" | "j2k" | "jp2"
        )
}

fn number_parts(path: &Path) -> Option<(String, String, usize, i64)> {
    let stem = path.file_stem()?.to_str()?;
    let end = stem.rfind(|c: char| c.is_ascii_digit())? + 1;
    let start = stem[..end]
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .len();
    Some((
        stem[..start].to_owned(),
        format!("{}.{}", &stem[end..], path.extension()?.to_str()?),
        end - start,
        stem[start..end].parse().ok()?,
    ))
}

fn sequence(path: &Path) -> Result<Vec<(i64, PathBuf)>, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid file name")?;
    let pattern = if let Some(start) = filename.find('#') {
        let end = start + filename[start..].chars().take_while(|&c| c == '#').count();
        Some((
            filename[..start].to_owned(),
            filename[end..].to_owned(),
            end - start,
        ))
    } else if let Some(start) = filename.find('%') {
        let end = filename[start..]
            .find('d')
            .map(|n| start + n)
            .ok_or("Invalid sequence pattern")?;
        let width = filename[start + 1..end]
            .trim_start_matches('0')
            .parse::<usize>()
            .unwrap_or(0);
        Some((
            filename[..start].to_owned(),
            filename[end + 1..].to_owned(),
            width,
        ))
    } else {
        None
    };
    let parts = number_parts(path);
    if pattern.is_none() && parts.is_none() {
        return Ok(vec![(0, path.to_owned())]);
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let candidate = entry.path();
        if !candidate.is_file() {
            continue;
        }
        if let Some((prefix, suffix, width)) = &pattern {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(digits) = name
                .strip_prefix(prefix.as_str())
                .and_then(|s| s.strip_suffix(suffix.as_str()))
                && (*width == 0
                    || digits.len() == *width
                    || (digits.len() > *width && !digits.starts_with('0')))
                && !digits.is_empty()
                && digits.bytes().all(|c| c.is_ascii_digit())
                && let Ok(number) = digits.parse()
            {
                files.push((number, candidate));
            }
        } else if let (Some((prefix, suffix, width, original)), Some((p, s, w, number))) =
            (&parts, number_parts(&candidate))
            && prefix == &p
            && suffix == &s
            && (width == &w
                || (*width == original.to_string().len() && w == number.to_string().len()))
        {
            files.push((number, candidate));
        }
        if files.len() > 50000 {
            return Err("Sequence exceeds the configured 50000-frame limit".into());
        }
    }
    files.sort_by_key(|item| item.0);
    if files.is_empty() {
        return Err("No matching sequence frames".into());
    }
    Ok(files)
}

fn probe(path: &Path) -> Result<Value, String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_streams", "-show_format", "-of", "json"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Cannot start ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("Invalid ffprobe response: {e}"))
}

pub fn inspect(path: &Path) -> Result<Media, String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let is_sequence = image_extension(&path)
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains(['#', '%']));
    let pairs = if is_sequence {
        sequence(&path)?
    } else {
        Vec::new()
    };
    let first = pairs.first().map(|item| item.1.as_path()).unwrap_or(&path);
    let dimensions = if is_sequence {
        image::ImageReader::open(first)
            .ok()
            .and_then(|reader| reader.with_guessed_format().ok())
            .and_then(|reader| reader.into_dimensions().ok())
    } else {
        None
    };
    let metadata = if dimensions.is_some() {
        Value::Null
    } else {
        probe(first)?
    };
    let stream = &metadata["streams"][0];
    let (width, height) = match dimensions {
        Some(dimensions) => dimensions,
        None => (
            stream["width"].as_u64().ok_or("Missing image width")? as u32,
            stream["height"].as_u64().ok_or("Missing image height")? as u32,
        ),
    };
    if width == 0 || height == 0 || width > 32768 || height > 32768 {
        return Err("Unsupported image dimensions".into());
    }
    let rate = if is_sequence {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exr"))
        {
            exr_rate(first).unwrap_or_default()
        } else {
            Rate::default()
        }
    } else {
        stream["avg_frame_rate"]
            .as_str()
            .and_then(Rate::parse)
            .or_else(|| stream["r_frame_rate"].as_str().and_then(Rate::parse))
            .unwrap_or_default()
    };
    let count = if is_sequence {
        pairs.len()
    } else {
        let number = stream["nb_frames"]
            .as_str()
            .and_then(|s| s.parse::<usize>().ok());
        match number.filter(|&n| n > 0) {
            Some(count) => count,
            None => {
                let duration = stream["duration"]
                    .as_str()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|duration| duration.is_finite() && *duration > 0.0)
                    .or_else(|| {
                        metadata["format"]["duration"]
                            .as_str()
                            .and_then(|value| value.parse::<f64>().ok())
                            .filter(|duration| duration.is_finite() && *duration > 0.0)
                    })
                    .ok_or("Cannot determine video duration without scanning the entire file")?;
                (duration * rate.value()).round().max(1.0) as usize
            }
        }
    };
    if count == 0 || count > 100_000_000 {
        return Err("Unsupported frame count".into());
    }
    Ok(Media {
        path,
        width,
        height,
        rate,
        count,
        numbers: pairs.iter().map(|item| item.0).collect(),
        files: pairs.into_iter().map(|item| item.1).collect(),
    })
}

fn exr_rate(path: &Path) -> Option<Rate> {
    let metadata = exr::meta::MetaData::read_from_file(path, false).ok()?;
    let (numerator, denominator) = metadata.headers.first()?.own_attributes.frames_per_second?;
    Rate::parse(&format!("{numerator}/{denominator}"))
}

struct VideoPipe {
    child: Child,
    stdout: ChildStdout,
    next: usize,
}

impl Drop for VideoPipe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Decoder {
    media: Media,
    pipe: Option<VideoPipe>,
}

impl Decoder {
    fn read(&mut self, index: usize) -> Result<Frame, String> {
        if !self.media.files.is_empty() {
            return decode_image(&self.media.files[index]);
        }
        if self.pipe.as_ref().is_none_or(|pipe| pipe.next != index) {
            self.pipe = None;
            let mut child = Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-threads",
                    "4",
                    "-noautorotate",
                    "-i",
                ])
                .arg(&self.media.path)
                .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf"])
                .arg(format!("select=gte(n\\,{index})"))
                .args([
                    "-vsync", "0", "-threads", "4", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("Cannot start ffmpeg: {e}"))?;
            let stdout = child.stdout.take().ok_or("Cannot read ffmpeg output")?;
            self.pipe = Some(VideoPipe {
                child,
                stdout,
                next: index,
            });
        }
        let pipe = self.pipe.as_mut().ok_or("Missing decoder")?;
        let size = self.media.width as usize * self.media.height as usize * 4;
        let mut pixels = vec![0; size];
        pipe.stdout
            .read_exact(&mut pixels)
            .map_err(|e| format!("Video decoding failed: {e}"))?;
        pipe.next += 1;
        Ok(Frame {
            width: self.media.width,
            height: self.media.height,
            pixels: Pixels::Bytes(pixels),
            gamma: 1.0,
            smooth_preview: false,
        })
    }
}

fn decode_image(path: &Path) -> Result<Frame, String> {
    let decoded = image::open(path);
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exr"))
        && let Err(error) = &decoded
    {
        return Err(format!("EXR decoding failed: {error}"));
    }
    if let Ok(image) = decoded {
        let width = image.width();
        let height = image.height();
        let gamma = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exr"))
        {
            2.2
        } else {
            1.0
        };
        let pixels = match image {
            image::DynamicImage::ImageRgb8(_)
            | image::DynamicImage::ImageRgba8(_)
            | image::DynamicImage::ImageLuma8(_)
            | image::DynamicImage::ImageLumaA8(_) => Pixels::Bytes(image.into_rgba8().into_raw()),
            _ => Pixels::Float(image.into_rgba32f().into_raw()),
        };
        return Ok(Frame {
            width,
            height,
            pixels,
            gamma,
            smooth_preview: false,
        });
    }
    let metadata = probe(path)?;
    let stream = &metadata["streams"][0];
    let width = stream["width"].as_u64().ok_or("Missing image width")? as u32;
    let height = stream["height"].as_u64().ok_or("Missing image height")? as u32;
    if width == 0 || height == 0 || width > 32768 || height > 32768 {
        return Err("Unsupported image dimensions".into());
    }
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "4",
            "-noautorotate",
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-threads",
            "4",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba64le",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    if output.stdout.len() != width as usize * height as usize * 8 {
        return Err("Incomplete image data".into());
    }
    let pixels = output
        .stdout
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]) as f32 / 65535.0)
        .collect();
    Ok(Frame {
        width,
        height,
        pixels: Pixels::Float(pixels),
        gamma: 1.0,
        smooth_preview: false,
    })
}

pub fn check_media(path: &Path) -> Result<(), String> {
    let media = inspect(path)?;
    println!(
        "{}: {}x{}, {} frames, {} FPS",
        media.path.display(),
        media.width,
        media.height,
        media.count,
        media.rate.label()
    );
    let mut decoder = Decoder { media, pipe: None };
    for index in [0, decoder.media.count - 1, 0] {
        let frame = decoder.read(index)?;
        println!(
            "Frame {}: {} decoded bytes",
            decoder.media.number(index),
            frame.bytes()
        );
    }
    std::io::stdout().flush().map_err(|e| e.to_string())
}
