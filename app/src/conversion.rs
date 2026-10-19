use crate::files::{self, Entry};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

pub fn convert(entry: &Entry, format: Format) -> Result<PathBuf, String> {
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
    let result = encode(entry, &source, &destination, format, sequence);
    if result.is_err() {
        if sequence {
            let _ = fs::remove_dir_all(&destination);
        } else {
            let _ = fs::remove_file(&destination);
        }
    }
    result.map(|()| destination)
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
) -> Result<(), String> {
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
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
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot start FFmpeg: {error}"))?;
    let input_result = if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(manifest.as_bytes())
    } else {
        Ok(())
    };
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "FFmpeg conversion failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    input_result.map_err(|error| error.to_string())
}
