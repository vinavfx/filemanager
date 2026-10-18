use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

const ICON_SIZE: u32 = 96;
const CACHE_CAPACITY: usize = 128;

type Sections = HashMap<String, HashMap<String, String>>;

fn sections(text: &str) -> Sections {
    let mut result = Sections::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            result
                .entry(section.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    result
}

fn active_theme() -> String {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let schema = if desktop.contains("mate") {
        "org.mate.interface"
    } else {
        "org.gnome.desktop.interface"
    };
    if let Ok(output) = Command::new("gsettings")
        .args(["get", schema, "icon-theme"])
        .stderr(Stdio::null())
        .output()
    {
        let theme = String::from_utf8_lossy(&output.stdout)
            .trim()
            .trim_matches('\'')
            .to_owned();
        if output.status.success() && !theme.is_empty() && !desktop.contains("kde") {
            return theme;
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let config =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    for (path, section, key) in [
        (config.join("kdeglobals"), "Icons", "Theme"),
        (
            config.join("gtk-3.0/settings.ini"),
            "Settings",
            "gtk-icon-theme-name",
        ),
    ] {
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(theme) = sections(&text)
                .get(section)
                .and_then(|values| values.get(key))
            {
                return theme.clone();
            }
        }
    }
    "hicolor".to_owned()
}

fn icon_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let data =
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from);
    let mut roots = vec![data.join("icons"), home.join(".icons")];
    let directories =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(std::env::split_paths(&directories).map(|path| path.join("icons")));
    roots
}

fn directory_distance(values: Option<&HashMap<String, String>>) -> u32 {
    let number = |key: &str, fallback| {
        values
            .and_then(|values| values.get(key))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(fallback)
    };
    let size = number("Size", ICON_SIZE);
    let scale = number("Scale", 1).max(1);
    let kind = values
        .and_then(|values| values.get("Type"))
        .map(String::as_str);
    let (minimum, maximum) = match kind {
        Some("Scalable") => (number("MinSize", size), number("MaxSize", size)),
        Some("Fixed") => (size, size),
        _ => {
            let threshold = number("Threshold", 2);
            (
                size.saturating_sub(threshold),
                size.saturating_add(threshold),
            )
        }
    };
    let minimum = minimum.saturating_mul(scale);
    let maximum = maximum.saturating_mul(scale).max(minimum);
    ICON_SIZE.abs_diff(ICON_SIZE.clamp(minimum, maximum))
}

struct Resolver {
    themes: Vec<Vec<PathBuf>>,
    roots: Vec<PathBuf>,
    rendered: HashMap<PathBuf, Option<egui::ColorImage>>,
    temporary: PathBuf,
}

impl Resolver {
    fn new() -> Self {
        let roots = icon_roots();
        let mut themes = Vec::new();
        let mut visited = HashSet::new();
        Self::add_theme(&active_theme(), &roots, &mut visited, &mut themes);
        Self::add_theme("hicolor", &roots, &mut visited, &mut themes);
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map_or_else(|| home.join(".cache"), PathBuf::from)
            .join("still-files/icons");
        Self {
            themes,
            roots,
            rendered: HashMap::new(),
            temporary: cache.join(format!("render-{}.png", std::process::id())),
        }
    }

    fn add_theme(
        name: &str,
        roots: &[PathBuf],
        visited: &mut HashSet<String>,
        themes: &mut Vec<Vec<PathBuf>>,
    ) {
        if visited.len() >= 16 || !visited.insert(name.to_owned()) {
            return;
        }
        let Some(text) = roots
            .iter()
            .find_map(|root| fs::read_to_string(root.join(name).join("index.theme")).ok())
        else {
            return;
        };
        let sections = sections(&text);
        let Some(theme) = sections.get("Icon Theme") else {
            return;
        };
        let mut directories: Vec<_> = ["Directories", "ScaledDirectories"]
            .iter()
            .filter_map(|key| theme.get(*key))
            .flat_map(|value| value.split(',').map(str::trim))
            .filter(|directory| !directory.is_empty())
            .collect();
        directories.sort_by_key(|directory| {
            (
                directory.contains("symbolic"),
                directory_distance(sections.get(*directory)),
            )
        });
        themes.push(
            directories
                .iter()
                .flat_map(|directory| {
                    roots
                        .iter()
                        .map(move |root| root.join(name).join(directory))
                })
                .collect(),
        );
        if let Some(inherits) = theme.get("Inherits") {
            for parent in inherits
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                Self::add_theme(parent, roots, visited, themes);
            }
        }
    }

    fn lookup(&self, names: &[String]) -> Option<PathBuf> {
        for directories in &self.themes {
            for name in names {
                for directory in directories {
                    for extension in ["png", "svg", "xpm"] {
                        let path = directory.join(format!("{name}.{extension}"));
                        if path.is_file() {
                            return Some(path);
                        }
                    }
                }
            }
        }
        for name in names {
            for directory in self
                .roots
                .iter()
                .chain(std::iter::once(&PathBuf::from("/usr/share/pixmaps")))
            {
                for extension in ["png", "svg", "xpm"] {
                    let path = directory.join(format!("{name}.{extension}"));
                    if path.is_file() {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    fn load(&mut self, path: &Path) -> Option<egui::ColorImage> {
        let output = Command::new("gio")
            .args(["info", "-a", "standard::icon", "--"])
            .arg(path)
            .env("LC_ALL", "C")
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut names: Vec<_> = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("standard::icon: "))?
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .map(str::to_owned)
            .collect();
        names.push("text-x-generic".to_owned());
        let icon = self.lookup(&names)?;
        if let Some(image) = self.rendered.get(&icon) {
            return image.clone();
        }
        let image = self.render(&icon);
        if self.rendered.len() >= CACHE_CAPACITY {
            self.rendered.clear();
        }
        self.rendered.insert(icon, image.clone());
        image
    }

    fn render(&self, path: &Path) -> Option<egui::ColorImage> {
        let image = if path.extension().is_some_and(|extension| extension == "png") {
            image::open(path).ok()?
        } else {
            let directory = self.temporary.parent()?;
            let key = crate::images::fingerprint(path).ok()?;
            let cached = directory.join(format!("{key:016x}-{ICON_SIZE}.png"));
            if let Ok(image) = image::open(&cached) {
                let image = image.into_rgba8();
                return Some(egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ));
            }
            fs::create_dir_all(directory).ok()?;
            let status = Command::new("gdk-pixbuf-thumbnailer")
                .args(["-s", "96"])
                .arg(path)
                .arg(&self.temporary)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok();
            let image = if status.is_some_and(|status| status.success()) {
                image::open(&self.temporary).ok()
            } else {
                None
            };
            if image.is_none() || fs::rename(&self.temporary, &cached).is_err() {
                let _ = fs::remove_file(&self.temporary);
            }
            image?
        };
        let image = image.thumbnail(ICON_SIZE, ICON_SIZE).into_rgba8();
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ))
    }
}

pub fn load(path: &Path) -> Option<egui::ColorImage> {
    Resolver::new().load(path)
}

pub struct Icons {
    sender: mpsc::SyncSender<(String, PathBuf)>,
    receiver: mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    pending: HashSet<String>,
    cache: HashMap<String, Option<egui::TextureHandle>>,
    order: VecDeque<String>,
}

impl Icons {
    pub fn new(context: egui::Context) -> Self {
        let (sender, requests) = mpsc::sync_channel::<(String, PathBuf)>(16);
        let (results, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut resolver = Resolver::new();
            while let Ok((key, path)) = requests.recv() {
                let image = if let Some(name) = key.strip_prefix("icon:") {
                    resolver
                        .lookup(&[name.to_owned()])
                        .and_then(|path| resolver.render(&path))
                } else {
                    resolver.load(&path)
                };
                if results.send((key, image)).is_err() {
                    break;
                }
                context.request_repaint();
            }
        });
        Self {
            sender,
            receiver,
            pending: HashSet::new(),
            cache: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn poll(&mut self, context: &egui::Context) {
        for _ in 0..2 {
            let Ok((key, image)) = self.receiver.try_recv() else {
                return;
            };
            self.pending.remove(&key);
            if self.cache.len() >= CACHE_CAPACITY {
                if let Some(oldest) = self.order.pop_front() {
                    self.cache.remove(&oldest);
                }
            }
            let texture = image.map(|image| {
                context.load_texture(
                    format!("system-icon:{key}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.order.push_back(key.clone());
            self.cache.insert(key, texture);
        }
        context.request_repaint_after(std::time::Duration::from_millis(16));
    }

    pub fn textures(&self) -> impl Iterator<Item = &egui::TextureHandle> {
        self.cache.values().filter_map(Option::as_ref)
    }

    fn key(path: &Path) -> Option<String> {
        Some(if let Some(extension) = path.extension() {
            format!("extension:{}", extension.to_string_lossy().to_lowercase())
        } else {
            format!("name:{}", path.file_name()?.to_string_lossy())
        })
    }

    pub fn ready(&self, path: &Path) -> bool {
        Self::key(path).is_none_or(|key| self.cache.contains_key(&key))
    }

    pub fn button(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        icon: &str,
        enabled: bool,
    ) -> egui::Response {
        let texture = self.request(format!("icon:{icon}"), Path::new(""));
        let button = if let Some(texture) = texture {
            egui::Button::image_and_text((texture.id(), egui::vec2(16.0, 16.0)), label)
        } else {
            egui::Button::new(format!("   {label}"))
        };
        ui.add_enabled(enabled, button)
    }

    pub fn get(&mut self, path: &Path) -> Option<&egui::TextureHandle> {
        self.request(Self::key(path)?, path)
    }

    fn request(&mut self, key: String, path: &Path) -> Option<&egui::TextureHandle> {
        if !self.cache.contains_key(&key)
            && !self.pending.contains(&key)
            && self
                .sender
                .try_send((key.clone(), path.to_path_buf()))
                .is_ok()
        {
            self.pending.insert(key.clone());
        }
        self.cache.get(&key).and_then(Option::as_ref)
    }
}
