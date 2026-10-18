use eframe::egui;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use url::Url;

#[derive(Clone)]
pub struct Bookmark {
    pub uri: String,
    pub path: Option<PathBuf>,
    pub label: String,
    pub exists: bool,
}

type BookmarkResult = io::Result<(String, Vec<Bookmark>)>;

pub struct Bookmarks {
    pub entries: Vec<Bookmark>,
    path: PathBuf,
    legacy: PathBuf,
    content: String,
    checked: Instant,
    receiver: Option<mpsc::Receiver<BookmarkResult>>,
}

impl Bookmarks {
    pub fn new(home: &Path) -> Self {
        let config =
            std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        Self {
            entries: Vec::new(),
            path: config.join("gtk-3.0/bookmarks"),
            legacy: home.join(".gtk-bookmarks"),
            content: String::new(),
            checked: Instant::now(),
            receiver: None,
        }
    }

    pub fn reload(&mut self) -> io::Result<()> {
        self.receiver = None;
        let (content, entries) = Self::read(&self.path, &self.legacy)?;
        self.content = content;
        self.entries = entries;
        Ok(())
    }

    fn read(path: &Path, legacy: &Path) -> BookmarkResult {
        let content = match std::fs::read_to_string(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match std::fs::read_to_string(legacy) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                    result => result?,
                }
            }
            result => result?,
        };
        let entries = content
            .lines()
            .filter_map(|line| {
                let (uri, label) = line
                    .split_once(' ')
                    .map_or((line, None), |(uri, label)| (uri, Some(label)));
                let url = Url::parse(uri).ok()?;
                let path = url.to_file_path().ok();
                let label = label.map(str::to_owned).unwrap_or_else(|| {
                    path.as_ref().and_then(|path| path.file_name()).map_or_else(
                        || uri.to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                });
                Some(Bookmark {
                    uri: uri.to_owned(),
                    exists: path.as_ref().is_none_or(|path| path.exists()),
                    path,
                    label,
                })
            })
            .collect();
        Ok((content, entries))
    }

    pub fn poll(&mut self, context: &egui::Context) -> io::Result<()> {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    let (content, entries) = result?;
                    self.content = content;
                    self.entries = entries;
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    return Err(io::Error::other("Bookmark reader stopped unexpectedly."));
                }
            }
        }
        if self.checked.elapsed() < Duration::from_secs(1) {
            return Ok(());
        }
        self.checked = Instant::now();
        let path = self.path.clone();
        let legacy = self.legacy.clone();
        let context = context.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(Self::read(&path, &legacy));
            context.request_repaint();
        });
        Ok(())
    }

    pub fn add(&mut self, path: &Path) -> io::Result<()> {
        let uri = Url::from_file_path(path)
            .map_err(|()| io::Error::new(io::ErrorKind::InvalidInput, "Invalid bookmark path"))?;
        self.reload()?;
        if self
            .entries
            .iter()
            .any(|bookmark| bookmark.path.as_deref() == Some(path))
        {
            return Ok(());
        }
        let mut content = self.content.clone();
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(uri.as_str());
        content.push('\n');
        self.write(&content)
    }

    pub fn remove(&mut self, uri: &str) -> io::Result<()> {
        self.reload()?;
        let content: String = self
            .content
            .split_inclusive('\n')
            .filter(|line| line.trim_end_matches(['\r', '\n']).split(' ').next() != Some(uri))
            .collect();
        if content == self.content {
            return Ok(());
        }
        self.write(&content)
    }

    pub fn restore(&mut self, bookmark: &Bookmark, index: usize) -> io::Result<()> {
        self.reload()?;
        if self.entries.iter().any(|entry| entry.uri == bookmark.uri) {
            return Ok(());
        }
        let mut lines: Vec<_> = self.content.lines().map(str::to_owned).collect();
        lines.insert(
            index.min(lines.len()),
            format!("{} {}", bookmark.uri, bookmark.label),
        );
        self.write(&(lines.join("\n") + "\n"))
    }

    fn write(&mut self, content: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temporary, content)?;
        if let Err(error) = std::fs::rename(&temporary, &self.path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        self.reload()
    }
}
