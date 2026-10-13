use eframe::egui;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
}

pub fn entries(path: &Path, hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let kind = item.file_type()?;
        let metadata = item.metadata().ok();
        entries.push(Entry {
            directory: item.path().is_dir(),
            symlink: kind.is_symlink(),
            size: metadata.map_or(0, |value| value.len()),
            path: item.path(),
            name,
        });
    }
    entries.sort_by(|a, b| {
        b.directory.cmp(&a.directory).then_with(|| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        })
    });
    Ok(entries)
}

pub fn image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "exr"
            )
        })
}

pub fn video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "mpg"
                    | "mpeg"
                    | "mpe"
                    | "mp4"
                    | "m4v"
                    | "mov"
                    | "mkv"
                    | "avi"
                    | "webm"
                    | "ogv"
                    | "mts"
                    | "m2ts"
                    | "ts"
                    | "mxf"
                    | "wmv"
                    | "flv"
                    | "vob"
                    | "3gp"
                    | "prores"
            )
        })
}

pub fn create_directory(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let mut components = Path::new(name).components();
    if name.trim().is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
        || Path::new(name).file_name() != Some(std::ffi::OsStr::new(name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a single folder name without path separators.",
        ));
    }
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn compress(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot compress the filesystem root.",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "The source has no file name.")
    })?;
    for index in 0.. {
        let mut archive_name = name.to_os_string();
        if index > 0 {
            archive_name.push(format!(" ({index})"));
        }
        archive_name.push(".zip");
        let archive = parent.join(archive_name);
        let output = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive)
        {
            Ok(output) => output,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = std::process::Command::new("zip")
            .args(["-q", "-r", "-y", "-", "--"])
            .arg(name)
            .current_dir(parent)
            .stdout(std::process::Stdio::from(output))
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|child| child.wait_with_output())
            .and_then(|output| {
                if output.status.success() {
                    Ok(archive.clone())
                } else {
                    Err(io::Error::other(format!(
                        "ZIP compression failed ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    )))
                }
            });
        if result.is_err() {
            fs::remove_file(&archive)?;
        }
        return result;
    }
    Err(io::Error::other("No available archive name."))
}

pub fn size_label(bytes: u64) -> String {
    let mut size = bytes as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut unit = 0;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", units[unit])
    }
}

pub struct SizeResult {
    pub bytes: u64,
    pub incomplete: bool,
}

pub struct SizeJob {
    pub receiver: mpsc::Receiver<SizeResult>,
    cancel: Arc<AtomicBool>,
}

impl Drop for SizeJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn folder_size(path: PathBuf, context: egui::Context) -> SizeJob {
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut pending = vec![path];
        let mut result = SizeResult {
            bytes: 0,
            incomplete: false,
        };
        while let Some(path) = pending.pop() {
            if worker_cancel.load(Ordering::Relaxed) {
                return;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(value) => value,
                Err(_) => {
                    result.incomplete = true;
                    continue;
                }
            };
            if !metadata.is_dir() {
                result.bytes = result.bytes.saturating_add(metadata.len());
                continue;
            }
            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        match entry {
                            Ok(entry) => pending.push(entry.path()),
                            Err(_) => result.incomplete = true,
                        }
                    }
                }
                Err(_) => result.incomplete = true,
            }
        }
        let _ = sender.send(result);
        context.request_repaint();
    });
    SizeJob { receiver, cancel }
}
