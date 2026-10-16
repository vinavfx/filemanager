use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Clone)]
pub struct Bookmark {
    pub uri: String,
    pub path: Option<PathBuf>,
    pub label: String,
    pub exists: bool,
}

pub struct Bookmarks {
    pub entries: Vec<Bookmark>,
    path: PathBuf,
    legacy: PathBuf,
    content: String,
    checked: Instant,
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
        }
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let content = match std::fs::read_to_string(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match std::fs::read_to_string(&self.legacy) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                    result => result?,
                }
            }
            result => result?,
        };
        if content != self.content {
            self.entries = content
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
                        path,
                        label,
                        exists: true,
                    })
                })
                .collect();
            self.content = content;
        }
        for bookmark in &mut self.entries {
            bookmark.exists = bookmark.path.as_ref().is_none_or(|path| path.exists());
        }
        Ok(())
    }

    pub fn poll(&mut self) -> io::Result<()> {
        if self.checked.elapsed() < Duration::from_secs(1) {
            return Ok(());
        }
        self.checked = Instant::now();
        self.reload()
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
