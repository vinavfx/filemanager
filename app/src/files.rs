use eframe::egui;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
    pub item_count: Option<usize>,
    pub modified: Option<SystemTime>,
    pub modified_label: String,
    pub sequence: Option<Arc<Vec<PathBuf>>>,
}

impl Entry {
    pub fn animated(&self) -> bool {
        self.sequence.is_some() || video_file(&self.path)
    }

    pub fn thumbnail(&self, frame: usize) -> (&Path, usize) {
        if let Some(paths) = &self.sequence {
            let index = if frame == crate::images::VIDEO_POSTER {
                paths.len() / 2
            } else {
                (paths.len() - 1) * frame.min(crate::images::VIDEO_FRAMES - 1)
                    / (crate::images::VIDEO_FRAMES - 1)
            };
            (&paths[index], 0)
        } else {
            (&self.path, if self.animated() { frame } else { 0 })
        }
    }
}

pub struct FolderCounts {
    sender: mpsc::Sender<(u64, PathBuf)>,
    receiver: mpsc::Receiver<(u64, PathBuf, usize)>,
    generation: Arc<AtomicU64>,
}

impl FolderCounts {
    pub fn new(context: egui::Context) -> Self {
        let (sender, requests) = mpsc::channel::<(u64, PathBuf)>();
        let (results, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&generation);
        std::thread::spawn(move || {
            while let Ok((generation, path)) = requests.recv() {
                if active.load(Ordering::Relaxed) != generation {
                    continue;
                }
                let Ok(entries) = fs::read_dir(&path) else {
                    continue;
                };
                let mut count = 0;
                let mut complete = true;
                for entry in entries {
                    if active.load(Ordering::Relaxed) != generation || entry.is_err() {
                        complete = false;
                        break;
                    }
                    count += 1;
                }
                if complete {
                    if results.send((generation, path, count)).is_err() {
                        break;
                    }
                    context.request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        });
        Self {
            sender,
            receiver,
            generation,
        }
    }

    pub fn request(&self, entries: &[Entry]) {
        let generation = self.generation.load(Ordering::Relaxed);
        for entry in entries.iter().filter(|entry| entry.directory) {
            let _ = self.sender.send((generation, entry.path.clone()));
        }
    }

    pub fn poll(&self) -> HashMap<PathBuf, usize> {
        let generation = self.generation.load(Ordering::Relaxed);
        self.receiver
            .try_iter()
            .filter_map(|(requested, path, count)| {
                (requested == generation).then_some((path, count))
            })
            .collect()
    }

    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for FolderCounts {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct ReadBudget {
    pub remaining: usize,
    pub deadline: Instant,
}

impl ReadBudget {
    fn check(&mut self) -> io::Result<()> {
        if self.remaining == 0 || Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Directory read budget exhausted.",
            ));
        }
        self.remaining -= 1;
        Ok(())
    }
}

pub fn entries(path: &Path, hidden: bool, sequences: bool) -> io::Result<Vec<Entry>> {
    read_entries(path, hidden, sequences, None)
}

pub fn entries_with_budget(
    path: &Path,
    hidden: bool,
    sequences: bool,
    budget: &mut ReadBudget,
) -> io::Result<Vec<Entry>> {
    read_entries(path, hidden, sequences, Some(budget))
}

fn read_entries(
    path: &Path,
    hidden: bool,
    sequences: bool,
    mut budget: Option<&mut ReadBudget>,
) -> io::Result<Vec<Entry>> {
    if let Some(budget) = budget.as_deref_mut() {
        budget.check()?;
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        if let Some(budget) = budget.as_deref_mut() {
            budget.check()?;
        }
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !hidden && (name.starts_with('.') || name.ends_with('~')) {
            continue;
        }
        let kind = item.file_type()?;
        let metadata = if kind.is_symlink() {
            fs::metadata(item.path()).ok()
        } else {
            item.metadata().ok()
        };
        let directory = kind.is_dir()
            || (kind.is_symlink() && metadata.as_ref().is_some_and(|value| value.is_dir()));
        entries.push(Entry {
            directory,
            symlink: kind.is_symlink(),
            size: metadata.as_ref().map_or(0, |value| value.len()),
            item_count: None,
            modified: metadata.and_then(|value| value.modified().ok()),
            modified_label: "—".to_owned(),
            sequence: None,
            path: item.path(),
            name,
        });
    }
    if sequences {
        entries = group_sequences(entries);
    }
    format_modified(&mut entries);
    entries.sort_by_cached_key(|entry| {
        (
            !entry.directory,
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    Ok(entries)
}

fn format_modified(entries: &mut [Entry]) -> Option<()> {
    let mut timestamps = String::new();
    let mut indices = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Some(modified) = entry.modified {
            let seconds = match modified.duration_since(UNIX_EPOCH) {
                Ok(duration) => i128::from(duration.as_secs()),
                Err(error) => {
                    -i128::from(error.duration().as_secs())
                        - i128::from(error.duration().subsec_nanos() > 0)
                }
            };
            timestamps.push_str(&format!("@{seconds}\n"));
            indices.push(index);
        }
    }
    if indices.is_empty() {
        return Some(());
    }
    let mut child = Command::new("date")
        .args(["-f", "-", "+%Y-%m-%d %H:%M"])
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut input = child.stdin.take()?;
    let writer = std::thread::spawn(move || input.write_all(timestamps.as_bytes()));
    let output = child.wait_with_output().ok()?;
    writer.join().ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    for (index, label) in indices
        .into_iter()
        .zip(String::from_utf8_lossy(&output.stdout).lines())
    {
        entries[index].modified_label = label.to_owned();
    }
    Some(())
}

fn sequence_number(entry: &Entry) -> Option<(String, String, u64)> {
    if entry.directory || !image_file(&entry.path) {
        return None;
    }
    let stem = entry.path.file_stem()?.to_str()?;
    let end = stem.rfind(|character: char| character.is_ascii_digit())? + 1;
    let start = stem[..end]
        .rfind(|character: char| !character.is_ascii_digit())
        .map_or(0, |index| {
            index + stem[index..].chars().next().map_or(0, char::len_utf8)
        });
    let digits = &stem[start..end];
    Some((
        stem[..start].to_owned(),
        format!("{}.{}", &stem[end..], entry.path.extension()?.to_str()?),
        digits.parse().ok()?,
    ))
}

fn group_sequences(entries: Vec<Entry>) -> Vec<Entry> {
    let mut groups: HashMap<(String, String), Vec<(u64, Entry)>> = HashMap::new();
    let mut result = Vec::new();
    for entry in entries {
        if let Some((prefix, suffix, number)) = sequence_number(&entry) {
            groups
                .entry((prefix, suffix))
                .or_default()
                .push((number, entry));
        } else {
            result.push(entry);
        }
    }
    for ((prefix, suffix), mut group) in groups {
        group.sort_by(|(a, entry_a), (b, entry_b)| {
            a.cmp(b).then_with(|| entry_a.name.cmp(&entry_b.name))
        });
        let mut remaining = group.as_slice();
        while !remaining.is_empty() {
            let length = remaining
                .windows(2)
                .position(|pair| pair[0].0.checked_add(1) != Some(pair[1].0))
                .map_or(remaining.len(), |index| index + 1);
            let run = &remaining[..length];
            let mut entry = run[0].1.clone();
            if length > 1 {
                entry.name = format!(
                    "{prefix}[{}–{}]{suffix} ({length} frames)",
                    run[0].0,
                    run[length - 1].0
                );
                entry.size = run
                    .iter()
                    .fold(0_u64, |size, (_, entry)| size.saturating_add(entry.size));
                entry.modified = run.iter().filter_map(|(_, entry)| entry.modified).max();
                entry.sequence = Some(Arc::new(
                    run.iter().map(|(_, entry)| entry.path.clone()).collect(),
                ));
            }
            result.push(entry);
            remaining = &remaining[length..];
        }
    }
    result
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

fn entry_path(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let mut components = Path::new(name).components();
    if name.trim().is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
        || Path::new(name).file_name() != Some(std::ffi::OsStr::new(name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a single name without path separators.",
        ));
    }
    Ok(parent.join(name))
}

pub fn create_directory(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let path = entry_path(parent, name)?;
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn rename_entry(source: &Path, name: &str) -> io::Result<PathBuf> {
    let parent = source.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot rename the root directory.",
        )
    })?;
    let target = entry_path(parent, name)?;
    if source == target {
        return Ok(target);
    }
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "An item with this name already exists.",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    gio(
        "move",
        &["-T", "-C"],
        &[source.as_os_str(), target.as_os_str()],
    )?;
    Ok(target)
}

pub(crate) fn gio(command: &str, options: &[&str], paths: &[&OsStr]) -> io::Result<Vec<u8>> {
    let output = Command::new("gio")
        .arg(command)
        .args(options)
        .arg("--")
        .args(paths)
        .env("LC_ALL", "C")
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
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
