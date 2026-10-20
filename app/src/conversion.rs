use crate::files::{self, Entry};
use eframe::egui;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

pub struct Job {
    pub destination: PathBuf,
    pub receiver: mpsc::Receiver<Result<Option<PathBuf>, String>>,
    state: Arc<State>,
    sources: Vec<PathBuf>,
}

struct State {
    cancelled: AtomicBool,
    progress: AtomicU32,
}

impl Job {
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn protects(&self, path: &Path) -> bool {
        self.destination.starts_with(path)
            || path.starts_with(&self.destination)
            || self.sources.iter().any(|source| source.starts_with(path))
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Relaxed)
    }

    pub fn progress(&self) -> Option<f32> {
        let progress = self.state.progress.load(Ordering::Relaxed);
        (progress != u32::MAX).then_some(progress as f32 / 1000.0)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Png,
    Mp4,
    Mov,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "Convert PNG",
            Self::Mp4 => "Convert MP4",
            Self::Mov => "Convert MOV",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }
}

pub fn start(entry: Entry, format: Format, context: egui::Context) -> Result<Job, String> {
    let source = entry
        .path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let parent = source
        .parent()
        .ok_or("The source has no parent directory")?;
    let stem = source.file_stem().ok_or("The source has no file name")?;
    let sequence =
        format == Format::Png && (entry.sequence.is_some() || files::video_file(&source));
    let destination =
        reserve_output(parent, stem, format, sequence).map_err(|error| error.to_string())?;
    let state = Arc::new(State {
        cancelled: AtomicBool::new(false),
        progress: AtomicU32::new(u32::MAX),
    });
    let (sender, receiver) = mpsc::channel();
    let job = Job {
        destination: destination.clone(),
        receiver,
        state: state.clone(),
        sources: entry
            .sequence
            .as_ref()
            .map_or_else(|| vec![source.clone()], |frames| frames.as_ref().clone()),
    };
    std::thread::spawn(move || {
        let result = encode(&entry, &source, &destination, format, sequence, &state);
        let was_cancelled = state.cancelled.load(Ordering::Relaxed);
        let result = if result.is_err() || was_cancelled {
            let cleanup = if sequence {
                fs::remove_dir_all(&destination)
            } else {
                fs::remove_file(&destination)
            };
            match cleanup {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("Cannot remove incomplete conversion: {error}"))
                }
                _ if was_cancelled => Ok(None),
                _ => result.map(|()| None),
            }
        } else {
            Ok(Some(destination))
        };
        let _ = sender.send(result);
        context.request_repaint();
    });
    Ok(job)
}

fn reserve_output(
    parent: &Path,
    stem: &std::ffi::OsStr,
    format: Format,
    sequence: bool,
) -> std::io::Result<PathBuf> {
    for index in 0.. {
        let mut name = stem.to_os_string();
        if sequence {
            name.push("_png");
        }
        if index > 0 {
            name.push(format!(" ({index})"));
        }
        if !sequence {
            name.push(format!(".{}", format.extension()));
        }
        let path = parent.join(name);
        let result = if sequence {
            fs::create_dir(&path)
        } else {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|_| ())
        };
        match result {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("No available output name"))
}

fn encode(
    entry: &Entry,
    source: &Path,
    destination: &Path,
    format: Format,
    sequence: bool,
    state: &Arc<State>,
) -> Result<(), String> {
    if state.cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let frames = entry.sequence.as_ref().map(|frames| frames.len());
    let duration = if let Some(frames) = frames {
        Some(frames as f64 / 24.0)
    } else {
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=duration:format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(source)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| {
                        line.parse::<f64>()
                            .ok()
                            .filter(|duration| duration.is_finite() && *duration > 0.0)
                    })
            })
    };
    if duration.is_some() {
        state.progress.store(0, Ordering::Relaxed);
    }
    if state.cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    let mut manifest = String::new();
    if let Some(frames) = &entry.sequence {
        for frame in frames.iter() {
            let path = frame.canonicalize().map_err(|error| error.to_string())?;
            let path = path.to_str().ok_or("Sequence paths must be valid UTF-8")?;
            if path.contains(['\n', '\r']) {
                return Err("Sequence paths cannot contain line breaks".to_owned());
            }
            manifest.push_str(&format!(
                "file 'file:{}'\nduration 0.0416666666666667\n",
                path.replace('\'', "'\\''")
            ));
        }
        command.args([
            "-protocol_whitelist",
            "file,pipe,fd",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            "pipe:0",
        ]);
    } else {
        command.arg("-i").arg(source);
    }
    command.args(["-map", "0:v:0", "-sn", "-dn"]);
    match format {
        Format::Png => {
            command.args(["-an", "-c:v", "png", "-vsync", "0"]);
            if !sequence {
                command.args(["-frames:v", "1", "-update", "1"]);
            }
        }
        Format::Mp4 => {
            command.args([
                "-map",
                "0:a?",
                "-c:v",
                "libx264",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ]);
        }
        Format::Mov => {
            command.args([
                "-map",
                "0:a?",
                "-c:v",
                "prores_ks",
                "-profile:v",
                "3",
                "-pix_fmt",
                "yuv422p10le",
                "-vf",
                "pad=ceil(iw/2)*2:ih",
                "-c:a",
                "pcm_s16le",
            ]);
        }
    }
    let output_path = if sequence {
        destination.join("frame_%06d.png")
    } else {
        destination.to_path_buf()
    };
    command
        .arg(output_path)
        .stdin(if entry.sequence.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot start FFmpeg: {error}"))?;
    let input = child
        .stdin
        .take()
        .map(|mut stdin| std::thread::spawn(move || stdin.write_all(manifest.as_bytes())));
    let stdout = child.stdout.take();
    let progress_state = state.clone();
    let progress = std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let fraction = match (key, frames, duration) {
                    ("frame", Some(total), _) if total > 0 => value
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .map(|frame| frame / total as f64),
                    ("out_time_us", None, Some(duration)) => value
                        .parse::<f64>()
                        .ok()
                        .map(|time| time / 1_000_000.0 / duration),
                    _ => None,
                };
                if let Some(fraction) = fraction {
                    progress_state.progress.fetch_max(
                        (fraction.clamp(0.0, 0.99) * 1000.0) as u32,
                        Ordering::Relaxed,
                    );
                }
            }
        }
    });
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes);
        }
        bytes
    });
    let status = loop {
        if state.cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    let _ = progress.join();
    let input_result = input.map(|thread| thread.join());
    let stderr = errors.join().unwrap_or_default();
    let status = status.map_err(|error| error.to_string())?;
    if state.cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    if !status.success() {
        return Err(format!(
            "FFmpeg conversion failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    if let Some(result) = input_result {
        result
            .map_err(|_| "Cannot write sequence input".to_owned())?
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
