mod bookmarks;
mod files;
mod images;
mod layout;
mod operations;
mod places;
mod search;

use eframe::egui::{self, Color32, Key, RichText, Stroke, Vec2};
use files::{Entry, SizeJob, SizeResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const ACCENT: Color32 = Color32::from_rgb(112, 168, 157);
const MUTED: Color32 = Color32::from_rgb(125, 132, 143);
const TREE_SIZE_WIDTH: f32 = 110.0;
const TREE_DATE_WIDTH: f32 = 156.0;
const PREVIEW_BAR_HEIGHT: f32 = 6.0;
const RECURSIVE_MAX_DEPTH: usize = 5;
const RECURSIVE_MAX_FOLDERS: usize = 64;
const RECURSIVE_MAX_ENTRIES: usize = 2_000;
const RECURSIVE_MAX_TIME: std::time::Duration = std::time::Duration::from_millis(250);

fn visual_accent(dark_mode: bool) -> Color32 {
    if dark_mode {
        Color32::from_rgb(195, 145, 245)
    } else {
        Color32::from_rgb(119, 55, 174)
    }
}

fn apply_theme(context: &egui::Context, dark_mode: bool) {
    let mut visuals = if dark_mode {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark_mode {
        visuals.panel_fill = Color32::from_rgb(25, 28, 34);
        visuals.window_fill = Color32::from_rgb(30, 34, 41);
        visuals.selection.bg_fill = Color32::from_rgb(49, 76, 73);
    } else {
        visuals.selection.bg_fill = Color32::from_rgb(191, 222, 215);
    }
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    context.set_visuals(visuals);
}

#[derive(Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum View {
    Tree,
    #[default]
    Grid,
    Media,
}

#[derive(Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum TreeSort {
    #[default]
    Name,
    Size,
    Modified,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    removed_places: Vec<PathBuf>,
    view: View,
    tree_sort: TreeSort,
    tree_descending: bool,
    zoom: f32,
    media_margin: u32,
    hidden: bool,
    sequences: bool,
    dark_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            removed_places: Vec::new(),
            view: View::Grid,
            tree_sort: TreeSort::Name,
            tree_descending: false,
            zoom: 1.0,
            media_margin: 4,
            hidden: false,
            sequences: true,
            dark_mode: true,
        }
    }
}

struct Disk {
    total: u64,
    available: u64,
    free: u64,
}

struct NameDialog {
    source: Option<PathBuf>,
    submit: bool,
    parent: PathBuf,
    name: String,
    focus: bool,
    error: Option<String>,
}

struct FileManager {
    settings: Settings,
    config: PathBuf,
    current: PathBuf,
    breadcrumb_path: PathBuf,
    thumbnails: images::Thumbnails,
    entries: Vec<Entry>,
    history: Vec<PathBuf>,
    operations: operations::History,
    position: usize,
    selected: Option<PathBuf>,
    last_selected: Option<PathBuf>,
    selection: HashSet<PathBuf>,
    selection_anchor: Option<PathBuf>,
    visual_line: bool,
    v_key_is_shortcut: Option<bool>,
    visible_paths: Vec<PathBuf>,
    scroll_to_selection: bool,
    page_scroll_delta: f32,
    pending_g: bool,
    sidebar_focused: bool,
    pending_window_command: bool,
    sidebar_cursor: Option<usize>,
    scroll_to_bookmark: bool,
    editing_path: bool,
    focus_path: bool,
    search: Option<search::Search>,
    address: String,
    expanded: HashSet<PathBuf>,
    tree_cache: HashMap<PathBuf, Vec<Entry>>,
    size_job: Option<SizeJob>,
    selection_size: Option<SizeResult>,
    disk_job: Option<mpsc::Receiver<Result<Disk, String>>>,
    disk: Option<Disk>,
    home_places: Vec<places::Place>,
    bookmarks: bookmarks::Bookmarks,
    new_folder: Option<NameDialog>,
    compression_job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    message: Option<String>,
}

impl FileManager {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*context.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 7.0);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        context.egui_ctx.set_style(style);
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join("still-files/settings.json");
        let (mut settings, config_error) = match std::fs::read(&config) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => (settings, None),
                Err(error) => (
                    Settings::default(),
                    Some(format!("Cannot read settings: {error}")),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Settings::default(), None)
            }
            Err(error) => (
                Settings::default(),
                Some(format!("Cannot read settings: {error}")),
            ),
        };
        apply_theme(&context.egui_ctx, settings.dark_mode);
        let home_places = places::defaults(&home);
        let mut bookmarks = bookmarks::Bookmarks::new(&home);
        let bookmark_error = bookmarks
            .reload()
            .err()
            .map(|error| format!("Cannot read shared bookmarks: {error}"));
        settings.zoom = if settings.zoom.is_finite() {
            settings.zoom.clamp(0.65, 3.6)
        } else {
            1.0
        };
        settings.media_margin = settings.media_margin.min(24);
        let desktop = home_places
            .iter()
            .find(|place| matches!(place.kind, places::Kind::Desktop))
            .map_or_else(|| home.clone(), |place| place.path.clone());
        let requested = std::env::args_os()
            .nth(1)
            .map_or_else(|| desktop.clone(), PathBuf::from);
        let current = requested.canonicalize().unwrap_or(requested);
        let current = if current.is_dir() { current } else { desktop };
        let mut app = Self {
            address: current.to_string_lossy().into_owned(),
            history: vec![current.clone()],
            operations: operations::History::default(),
            breadcrumb_path: current.clone(),
            thumbnails: images::Thumbnails::new(context.egui_ctx.clone()),
            current,
            config,
            settings,
            entries: Vec::new(),
            position: 0,
            selected: None,
            last_selected: None,
            selection: HashSet::new(),
            selection_anchor: None,
            visual_line: false,
            v_key_is_shortcut: None,
            visible_paths: Vec::new(),
            scroll_to_selection: false,
            page_scroll_delta: 0.0,
            pending_g: false,
            sidebar_focused: false,
            pending_window_command: false,
            sidebar_cursor: None,
            scroll_to_bookmark: false,
            editing_path: false,
            focus_path: false,
            search: None,
            expanded: HashSet::new(),
            tree_cache: HashMap::new(),
            size_job: None,
            selection_size: None,
            disk_job: None,
            disk: None,
            home_places,
            bookmarks,
            new_folder: None,
            compression_job: None,
            message: config_error.or(bookmark_error),
        };
        app.refresh(&context.egui_ctx);
        app
    }

    fn save(&mut self) {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            if let Some(parent) = self.config.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
            let mut bytes = Vec::new();
            let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
            self.settings.serialize(&mut serializer)?;
            let temporary = self
                .config
                .with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(temporary, &self.config)?;
            Ok(())
        })();
        if let Err(error) = result {
            self.message = Some(format!("Cannot save settings: {error}"));
        }
    }

    fn refresh(&mut self, context: &egui::Context) {
        match files::entries(&self.current, self.settings.hidden, self.settings.sequences) {
            Ok(entries) => self.entries = entries,
            Err(error) => {
                self.entries.clear();
                self.message = Some(format!("Cannot read directory: {error}"));
            }
        }
        self.tree_cache.clear();
        self.thumbnails.cancel();
        self.select(None, context);
        self.refresh_disk(context);
        if let Some(search) = self.search.take() {
            let mut replacement =
                search::Search::new(self.current.clone(), self.settings.hidden, context.clone());
            replacement.query.clone_from(&search.query);
            replacement.submit();
            self.search = Some(replacement);
        }
    }

    fn refresh_disk(&mut self, context: &egui::Context) {
        let path = self.current.clone();
        let context = context.clone();
        let (sender, receiver) = mpsc::channel();
        self.disk = None;
        self.disk_job = Some(receiver);
        std::thread::spawn(move || {
            let result = (|| -> std::io::Result<Disk> {
                Ok(Disk {
                    total: fs2::total_space(&path)?,
                    available: fs2::available_space(&path)?,
                    free: fs2::free_space(&path)?,
                })
            })()
            .map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn navigate(&mut self, path: PathBuf, context: &egui::Context) {
        match path.canonicalize().and_then(|path| {
            files::entries(&path, self.settings.hidden, self.settings.sequences)
                .map(|entries| (path, entries))
        }) {
            Ok((path, entries)) => {
                self.search = None;
                self.thumbnails.cancel();
                self.new_folder = None;
                if path != self.current {
                    self.history.truncate(self.position + 1);
                    self.history.push(path.clone());
                    self.position = self.history.len() - 1;
                }
                self.breadcrumb_path = path.clone();
                self.current = path;
                self.entries = entries;
                self.address = self.current.to_string_lossy().into_owned();
                self.expanded.clear();
                self.tree_cache.clear();
                self.select(None, context);
                self.refresh_disk(context);
            }
            Err(error) => self.message = Some(format!("Cannot open directory: {error}")),
        }
    }

    fn navigate_breadcrumb(&mut self, path: PathBuf, context: &egui::Context) {
        let trail = self.breadcrumb_path.clone();
        self.navigate(path.clone(), context);
        if self.current == path {
            self.breadcrumb_path = trail;
        }
    }

    fn history_step(&mut self, forward: bool, context: &egui::Context) {
        let next = if forward {
            (self.position + 1 < self.history.len()).then_some(self.position + 1)
        } else {
            self.position.checked_sub(1)
        };
        if let Some(next) = next {
            let path = self.history[next].clone();
            match files::entries(&path, self.settings.hidden, self.settings.sequences) {
                Ok(entries) => {
                    self.search = None;
                    self.thumbnails.cancel();
                    self.new_folder = None;
                    self.position = next;
                    if !self.breadcrumb_path.starts_with(&path) {
                        self.breadcrumb_path = path.clone();
                    }
                    self.current = path;
                    self.entries = entries;
                    self.address = self.current.to_string_lossy().into_owned();
                    self.expanded.clear();
                    self.tree_cache.clear();
                    self.select(None, context);
                    self.refresh_disk(context);
                }
                Err(error) => self.message = Some(format!("Cannot open directory: {error}")),
            }
        }
    }

    fn select(&mut self, path: Option<PathBuf>, context: &egui::Context) {
        self.visual_line = false;
        self.pending_g = false;
        self.size_job = None;
        self.selection_size = None;
        if let Some(path) = &path {
            self.last_selected = Some(path.clone());
            if path.is_dir() {
                self.size_job = Some(files::folder_size(path.clone(), context.clone()));
            } else if let Some(entry) = self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .find(|entry| &entry.path == path && entry.sequence.is_some())
            {
                self.selection_size = Some(SizeResult {
                    bytes: entry.size,
                    incomplete: false,
                });
            } else {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) => {
                        self.selection_size = Some(SizeResult {
                            bytes: metadata.len(),
                            incomplete: false,
                        })
                    }
                    Err(error) => self.message = Some(format!("Cannot read file size: {error}")),
                }
            }
        }
        self.selection = path.iter().cloned().collect();
        self.selection_anchor = path.clone();
        self.selected = path;
    }

    fn select_entry(&mut self, path: PathBuf, context: &egui::Context) {
        let visual_line = self.visual_line;
        if !visual_line && !context.input(|input| input.modifiers.shift) {
            self.select(Some(path), context);
            return;
        }
        let anchor = self
            .selection_anchor
            .clone()
            .unwrap_or_else(|| path.clone());
        let start = self.visible_paths.iter().position(|item| item == &anchor);
        let end = self.visible_paths.iter().position(|item| item == &path);
        self.select(Some(path), context);
        self.visual_line = visual_line;
        if let (Some(start), Some(end)) = (start, end) {
            self.selection = self.visible_paths[start.min(end)..=start.max(end)]
                .iter()
                .cloned()
                .collect();
            self.selection_anchor = Some(anchor);
            if self.selection.len() > 1 {
                self.size_job = None;
                self.selection_size = None;
            }
        }
    }

    fn add_bookmark(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if !path.is_dir() {
            return;
        }
        if self.home_places.iter().any(|place| place.path == path) {
            self.settings
                .removed_places
                .retain(|removed| removed != &path);
            self.save();
        }
        if let Err(error) = self.bookmarks.add(&path) {
            self.message = Some(format!("Cannot save shared bookmarks: {error}"));
        }
    }

    fn bookmark(&mut self) {
        match self.selected.clone() {
            Some(path) if path.is_dir() => self.add_bookmark(path),
            None => self.add_bookmark(self.current.clone()),
            Some(_) => {
                self.message = Some(
                    "Select a folder or clear the selection to bookmark the current directory."
                        .into(),
                )
            }
        }
    }

    fn open(&mut self, path: PathBuf, context: &egui::Context) {
        if path.is_dir() {
            self.navigate(path, context);
        } else if let Err(error) = std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map(|mut child| {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
        {
            self.message = Some(format!("Cannot open file: {error}"));
        }
    }

    fn open_terminal(&mut self, path: &Path) {
        if let Err(error) = std::process::Command::new("konsole")
            .arg("--separate")
            .arg("--workdir")
            .arg(path)
            .spawn()
            .map(|mut child| {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
        {
            self.message = Some(format!("Cannot open Konsole: {error}"));
        }
    }

    fn show_properties(&mut self, path: &Path) {
        let mut uri = String::from("file://");
        for &byte in path.as_os_str().as_encoded_bytes() {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
                uri.push(char::from(byte));
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
        if let Err(error) = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.mate.Caja",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItemProperties",
            ])
            .arg(format!("['{uri}']"))
            .arg("")
            .stdout(std::process::Stdio::null())
            .spawn()
            .map(|mut child| {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
        {
            self.message = Some(format!("Cannot open Caja properties: {error}"));
        }
    }

    fn directory_menu(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if ui.button("Open Konsole here").clicked() {
            self.open_terminal(&self.current.clone());
            ui.close_menu();
        }
        if ui.button("New folder…").clicked() {
            self.new_folder = Some(NameDialog {
                source: None,
                submit: false,
                parent: self.current.clone(),
                name: String::new(),
                focus: true,
                error: None,
            });
            ui.close_menu();
        }
        if ui.button("Bookmark this folder").clicked() {
            self.add_bookmark(self.current.clone());
            ui.close_menu();
        }
        if ui
            .add_enabled(
                self.compression_job.is_none(),
                egui::Button::new("Compress this folder to ZIP"),
            )
            .clicked()
        {
            self.compress(self.current.clone(), context);
            ui.close_menu();
        }
        if ui.button("Copy directory path").clicked() {
            context.copy_text(self.current.to_string_lossy().into_owned());
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Refresh · Ctrl+R / F5").clicked() {
            self.refresh(context);
            ui.close_menu();
        }
        if ui
            .checkbox(&mut self.settings.hidden, "Show hidden files")
            .changed()
        {
            self.refresh(context);
            self.save();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Properties").clicked() {
            self.show_properties(&self.current.clone());
            ui.close_menu();
        }
    }

    fn compress(&mut self, path: PathBuf, context: &egui::Context) {
        if self.compression_job.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        self.compression_job = Some(receiver);
        std::thread::spawn(move || {
            let result = files::compress(&path).map_err(|error| error.to_string());
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self, context: &egui::Context) {
        if let Some(search) = &mut self.search {
            search.poll();
        }
        self.thumbnails.poll(context);
        if let Some(job) = &self.size_job {
            if let Ok(result) = job.receiver.try_recv() {
                self.selection_size = Some(result);
                self.size_job = None;
            }
        }
        if let Some(job) = &self.disk_job {
            if let Ok(result) = job.try_recv() {
                match result {
                    Ok(disk) => self.disk = Some(disk),
                    Err(error) => self.message = Some(format!("Cannot read disk usage: {error}")),
                }
                self.disk_job = None;
            }
        }
        if let Some(result) = self
            .compression_job
            .as_ref()
            .and_then(|job| job.try_recv().ok())
        {
            self.compression_job = None;
            match result {
                Ok(path) => {
                    if path.parent() == Some(self.current.as_path()) {
                        self.refresh(context);
                    }
                    self.message = Some(format!("Archive created: {}", path.display()));
                }
                Err(error) => self.message = Some(format!("Cannot compress: {error}")),
            }
        }
    }

    fn pane_shortcuts(&mut self, context: &egui::Context) {
        if self.new_folder.is_some() {
            self.pending_window_command = false;
            return;
        }
        let mut sidebar = None;
        context.input_mut(|input| {
            input.events.retain(|event| {
                if sidebar.is_some()
                    && matches!(event, egui::Event::Text(text) if text.eq_ignore_ascii_case("h") || text.eq_ignore_ascii_case("l"))
                {
                    return false;
                }
                if let egui::Event::PointerButton { pressed: true, .. } = event {
                    self.pending_window_command = false;
                }
                if let egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    repeat: false,
                    ..
                } = event
                {
                    if *key == Key::W && modifiers.matches_logically(egui::Modifiers::CTRL) {
                        self.pending_window_command = true;
                        return false;
                    }
                    if std::mem::take(&mut self.pending_window_command) {
                        match key {
                            Key::H | Key::ArrowLeft => sidebar = Some(true),
                            Key::L | Key::ArrowRight => sidebar = Some(false),
                            _ => return true,
                        }
                        return false;
                    }
                }
                true
            });
        });
        if let Some(sidebar) = sidebar {
            self.sidebar_focused = sidebar;
            self.pending_g = false;
            self.editing_path = false;
            if sidebar {
                self.scroll_to_bookmark = true;
            } else if self.search.is_none() {
                if self.selected.is_none() {
                    self.select(self.visible_paths.first().cloned(), context);
                }
                self.scroll_to_selection = true;
            }
            if let Some(search) = &mut self.search {
                search.focus = !sidebar;
            }
            context.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
    }

    fn vim_shortcuts(&mut self, context: &egui::Context) {
        if self.search.is_some() || self.new_folder.is_some() || self.text_input_focused(context) {
            self.pending_g = false;
            return;
        }
        let mut view_step: i32 = 0;
        context.input_mut(|input| {
            input.events.retain_mut(|event| {
                if let egui::Event::PointerButton { pressed: true, .. } = event {
                    self.pending_g = false;
                }
                let egui::Event::Key {
                    key,
                    physical_key,
                    modifiers,
                    pressed: true,
                    repeat,
                    ..
                } = event
                else {
                    return true;
                };
                if *repeat {
                    return *key != Key::G;
                }
                if !modifiers.matches_exact(egui::Modifiers::NONE)
                    && !modifiers.matches_exact(egui::Modifiers::SHIFT)
                {
                    self.pending_g = false;
                    return true;
                }
                if *key == Key::G {
                    if modifiers.shift || self.pending_g {
                        *key = if modifiers.shift { Key::End } else { Key::Home };
                        *physical_key = None;
                        *modifiers = egui::Modifiers::NONE;
                        self.pending_g = false;
                        return true;
                    }
                    self.pending_g = true;
                    return false;
                }
                if *key == Key::T && self.pending_g {
                    view_step += if modifiers.shift { -1 } else { 1 };
                    self.pending_g = false;
                    return false;
                }
                self.pending_g = false;
                true
            });
        });
        if view_step != 0 {
            let index = match self.settings.view {
                View::Tree => 0,
                View::Grid => 1,
                View::Media => 2,
            };
            self.settings.view =
                [View::Tree, View::Grid, View::Media][(index + view_step).rem_euclid(3) as usize];
            self.scroll_to_selection = !self.sidebar_focused && self.selected.is_some();
            self.save();
        }
    }

    fn visual_line_shortcut(&mut self, context: &egui::Context) -> bool {
        let enabled = !self.sidebar_focused
            && self.search.is_none()
            && self.new_folder.is_none()
            && !self.text_input_focused(context);
        let mut toggle = false;
        context.input_mut(|input| {
            let ctrl = input.modifiers.ctrl;
            input.events.retain(|event| match event {
                egui::Event::Paste(_) if ctrl => {
                    if enabled && self.v_key_is_shortcut != Some(true) {
                        toggle = true;
                    }
                    self.v_key_is_shortcut = Some(enabled);
                    !enabled
                }
                egui::Event::Key {
                    key: Key::V,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    if *pressed {
                        if !repeat {
                            let shortcut = enabled && modifiers.ctrl;
                            toggle |= shortcut && self.v_key_is_shortcut != Some(true);
                            self.v_key_is_shortcut = Some(shortcut);
                        }
                    } else {
                        toggle |= self.v_key_is_shortcut.take().is_none() && enabled;
                    }
                    !(enabled && modifiers.ctrl)
                }
                egui::Event::WindowFocused(false) => {
                    self.v_key_is_shortcut = None;
                    true
                }
                _ => true,
            });
        });
        toggle
    }

    fn shortcuts(&mut self, context: &egui::Context) {
        self.pane_shortcuts(context);
        self.vim_shortcuts(context);
        let visual_line_shortcut = self.visual_line_shortcut(context);
        let scroll = context.input_mut(|input| {
            if input.modifiers.ctrl {
                let scroll = input.raw_scroll_delta.y;
                input.raw_scroll_delta = Vec2::ZERO;
                input.smooth_scroll_delta = Vec2::ZERO;
                scroll
            } else {
                0.0
            }
        });
        if scroll != 0.0 {
            let zoom = (self.settings.zoom * (scroll * 0.003).exp()).clamp(0.65, 3.6);
            if self.settings.zoom != zoom {
                self.settings.zoom = zoom;
                self.save();
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::F))
            || context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::P))
        {
            self.start_search(context);
        }
        if context.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::L)
        }) {
            self.search = None;
            self.address = self.current.to_string_lossy().into_owned();
            self.sidebar_focused = false;
            self.editing_path = true;
            self.focus_path = true;
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if self.new_folder.is_some() {
                self.new_folder = None;
            } else if self.search.is_some() {
                self.search = None;
                context.memory_mut(|memory| memory.surrender_focus(egui::Id::new("search-input")));
            } else if self.editing_path {
                self.editing_path = false;
            } else if self.visual_line {
                self.select(self.selected.clone(), context);
            } else {
                self.select(None, context);
            }
        }
        if self.new_folder.is_none() && !self.text_input_focused(context) {
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::U)) {
                let outcome = self.operations.undo();
                self.complete_operation(outcome, context);
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::R)) {
                let outcome = self.operations.redo();
                self.complete_operation(outcome, context);
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowLeft))
                || context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::O))
            {
                self.history_step(false, context);
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowRight))
                || context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::I))
            {
                self.history_step(true, context);
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowUp)) {
                if let Some(parent) = self.current.parent() {
                    self.navigate_breadcrumb(parent.to_path_buf(), context);
                }
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::F5)) {
                self.refresh(context);
            }
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::H)
            }) {
                self.settings.hidden = !self.settings.hidden;
                self.refresh(context);
                self.save();
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::H)) {
                if let Some(parent) = self.current.parent() {
                    self.navigate_breadcrumb(parent.to_path_buf(), context);
                }
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::L)) {
                let next = self
                    .breadcrumb_path
                    .ancestors()
                    .find(|path| path.parent() == Some(self.current.as_path()))
                    .map(Path::to_path_buf);
                if let Some(next) = next {
                    self.navigate_breadcrumb(next, context);
                }
            }
        }
        if self.sidebar_focused && self.new_folder.is_none() && !self.text_input_focused(context) {
            return;
        }
        if self.search.is_some() && self.new_folder.is_none() {
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::F5)) {
                self.refresh(context);
            }
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::H)
            }) {
                self.settings.hidden = !self.settings.hidden;
                self.refresh(context);
                self.save();
            }
            return;
        }
        if self.new_folder.is_some() || self.text_input_focused(context) {
            return;
        }
        if visual_line_shortcut {
            let enable = !self.visual_line;
            let path = self
                .selected
                .clone()
                .or_else(|| self.visible_paths.first().cloned());
            self.select(path, context);
            self.visual_line = enable && self.selected.is_some();
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::D)) {
            if self.visual_line {
                self.trash_selection(context);
            } else {
                self.bookmark();
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Delete)) {
            self.trash_selection(context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::N)) {
            self.start_rename();
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, Key::O))
            && self.settings.view == View::Tree
        {
            if let Some(path) = self.selected.clone() {
                if path.is_dir() && !path.is_symlink() {
                    self.toggle_recursive_expansion(path);
                }
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::O)) {
            if let Some(path) = self.selected.clone() {
                if self.settings.view == View::Tree && path.is_dir() && !path.is_symlink() {
                    if !self.expanded.remove(&path) {
                        self.expand_directory(&path);
                    }
                } else {
                    self.open(path, context);
                }
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter))
            || context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Space))
        {
            if let Some(path) = self.selected.clone() {
                self.open(path, context);
            }
        }
    }

    fn text_input_focused(&self, context: &egui::Context) -> bool {
        self.editing_path
            || context
                .memory(|memory| memory.focused())
                .is_some_and(|id| egui::text_edit::TextEditState::load(context, id).is_some())
    }

    fn navigation_jump(&mut self, count: usize, context: &egui::Context) -> Option<usize> {
        if count == 0 {
            return None;
        }
        context.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, Key::Home) {
                Some(0)
            } else if input.consume_key(egui::Modifiers::NONE, Key::End) {
                Some(count - 1)
            } else {
                None
            }
        })
    }

    fn navigate_items(
        &mut self,
        items: &[(Entry, egui::Pos2)],
        viewport_height: f32,
        context: &egui::Context,
    ) {
        self.visible_paths = items.iter().map(|(entry, _)| entry.path.clone()).collect();
        if self.sidebar_focused {
            return;
        }
        if self.search.is_some()
            || self.new_folder.is_some()
            || self.text_input_focused(context)
            || items.is_empty()
        {
            return;
        }
        if let Some(index) = self.navigation_jump(items.len(), context) {
            let path = items[index].0.path.clone();
            if self.visual_line {
                self.select_entry(path, context);
            } else {
                self.select(Some(path), context);
            }
            self.scroll_to_selection = true;
            return;
        }
        let page_direction = context.input_mut(|input| {
            if input.consume_key(egui::Modifiers::CTRL, Key::D) {
                1.0
            } else if input.consume_key(egui::Modifiers::CTRL, Key::U) {
                -1.0
            } else {
                0.0
            }
        });
        if page_direction != 0.0 {
            self.page_scroll_delta = page_direction * viewport_height / 2.0;
            let position = items
                .iter()
                .find(|(entry, _)| self.selected.as_ref() == Some(&entry.path))
                .unwrap_or(&items[0])
                .1;
            let target_y = position.y + self.page_scroll_delta;
            let target = items
                .iter()
                .filter(|(_, candidate)| (candidate.y - position.y) * page_direction >= 0.0)
                .min_by(|(_, a), (_, b)| {
                    (a.y - target_y)
                        .abs()
                        .total_cmp(&(b.y - target_y).abs())
                        .then_with(|| {
                            (a.x - position.x)
                                .abs()
                                .total_cmp(&(b.x - position.x).abs())
                        })
                });
            if let Some((entry, _)) = target {
                self.select_entry(entry.path.clone(), context);
            }
            self.scroll_to_selection = false;
            return;
        }
        let Some((key, _)) = [
            (Key::ArrowUp, Key::K),
            (Key::ArrowDown, Key::J),
            (Key::ArrowLeft, Key::H),
            (Key::ArrowRight, Key::L),
        ]
        .into_iter()
        .find(|(arrow, vim)| {
            context.input_mut(|input| {
                let modifiers = if input.modifiers.shift {
                    egui::Modifiers::SHIFT
                } else {
                    egui::Modifiers::NONE
                };
                input.consume_key(modifiers, *arrow) || input.consume_key(modifiers, *vim)
            })
        }) else {
            return;
        };
        let current = items
            .iter()
            .position(|(entry, _)| self.selected.as_ref() == Some(&entry.path));
        let target = if let Some(index) = current {
            let (entry, position) = &items[index];
            if self.settings.view == View::Tree {
                match key {
                    Key::ArrowRight if entry.directory && !entry.symlink => {
                        if !self.expanded.contains(&entry.path) {
                            self.expand_directory(&entry.path);
                            return;
                        }
                        items
                            .get(index + 1)
                            .filter(|(child, _)| child.path.parent() == Some(entry.path.as_path()))
                    }
                    Key::ArrowLeft => {
                        if self.expanded.remove(&entry.path) {
                            return;
                        }
                        items
                            .iter()
                            .find(|(parent, _)| entry.path.parent() == Some(parent.path.as_path()))
                    }
                    Key::ArrowUp => index.checked_sub(1).and_then(|index| items.get(index)),
                    Key::ArrowDown => items.get(index + 1),
                    _ => None,
                }
            } else {
                items
                    .iter()
                    .filter(|(_, candidate)| match key {
                        Key::ArrowUp => candidate.y < position.y,
                        Key::ArrowDown => candidate.y > position.y,
                        Key::ArrowLeft => candidate.y == position.y && candidate.x < position.x,
                        Key::ArrowRight => candidate.y == position.y && candidate.x > position.x,
                        _ => false,
                    })
                    .min_by(|(_, a), (_, b)| {
                        (a.y - position.y)
                            .abs()
                            .total_cmp(&(b.y - position.y).abs())
                            .then_with(|| {
                                (a.x - position.x)
                                    .abs()
                                    .total_cmp(&(b.x - position.x).abs())
                            })
                    })
            }
        } else {
            items
                .iter()
                .find(|(entry, _)| self.last_selected.as_ref() == Some(&entry.path))
                .or_else(|| {
                    if matches!(key, Key::ArrowUp | Key::ArrowLeft) {
                        items.last()
                    } else {
                        items.first()
                    }
                })
        };
        if let Some((entry, _)) = target {
            self.select_entry(entry.path.clone(), context);
            self.scroll_to_selection = true;
        }
    }

    fn entry_scroll_area(&mut self, ui: &egui::Ui, id: &str) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical().id_salt(id);
        let delta = std::mem::take(&mut self.page_scroll_delta);
        if delta != 0.0 {
            let offset = egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id(id))
                .map_or(0.0, |state| state.offset.y);
            scroll = scroll.vertical_scroll_offset((offset + delta).max(0.0));
        }
        scroll
    }

    fn expand_directory(&mut self, path: &Path) {
        match files::entries(path, self.settings.hidden, self.settings.sequences) {
            Ok(children) => {
                self.tree_cache.insert(path.to_path_buf(), children);
                self.expanded.insert(path.to_path_buf());
            }
            Err(error) => self.message = Some(format!("Cannot expand directory: {error}")),
        }
    }

    fn toggle_recursive_expansion(&mut self, path: PathBuf) {
        if self.expanded.contains(&path)
            && self
                .expanded
                .iter()
                .any(|child| child != &path && child.starts_with(&path))
        {
            self.expanded
                .retain(|child| child == &path || !child.starts_with(&path));
            return;
        }
        let root_depth = path
            .strip_prefix(&self.current)
            .map_or(0, |path| path.components().count().saturating_sub(1));
        let mut budget = files::ReadBudget {
            remaining: RECURSIVE_MAX_ENTRIES,
            deadline: std::time::Instant::now() + RECURSIVE_MAX_TIME,
        };
        let mut pending = vec![(path, root_depth)];
        let mut expanded = 0;
        let mut limited = false;
        while let Some((path, depth)) = pending.pop() {
            if depth >= 64 || depth - root_depth >= RECURSIVE_MAX_DEPTH {
                limited = true;
                continue;
            }
            if expanded >= RECURSIVE_MAX_FOLDERS {
                limited = true;
                break;
            }
            match files::entries_with_budget(
                &path,
                self.settings.hidden,
                self.settings.sequences,
                &mut budget,
            ) {
                Ok(children) => {
                    pending.extend(
                        children
                            .iter()
                            .filter(|entry| entry.directory && !entry.symlink)
                            .map(|entry| (entry.path.clone(), depth + 1)),
                    );
                    self.tree_cache.insert(path.clone(), children);
                    self.expanded.insert(path);
                    expanded += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    limited = true;
                    break;
                }
                Err(error) => self.message = Some(format!("Cannot expand directory: {error}")),
            }
        }
        if limited {
            self.message = Some(format!(
                "Recursive expansion limited to {RECURSIVE_MAX_DEPTH} levels, \
                 {RECURSIVE_MAX_FOLDERS} folders, {RECURSIVE_MAX_ENTRIES} scanned entries \
                 or {} ms. Open remaining folders manually.",
                RECURSIVE_MAX_TIME.as_millis()
            ));
        }
    }

    fn toolbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(context, |ui| {
            ui.add_space(4.0);
            let (row, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::hover());
            let navigation =
                egui::Rect::from_min_max(row.min, egui::pos2(row.left() + 176.0, row.bottom()));
            let options =
                egui::Rect::from_min_max(egui::pos2(row.right() - 396.0, row.top()), row.max);
            let address = egui::Rect::from_min_max(
                egui::pos2(navigation.right() + 16.0, row.top()),
                egui::pos2(options.left() - 16.0, row.bottom()),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(navigation)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    if arrow_button(ui, self.position > 0, Vec2::new(-1.0, 0.0))
                        .on_hover_text("Back · Alt+Left")
                        .clicked()
                    {
                        self.history_step(false, context);
                    }
                    if arrow_button(
                        ui,
                        self.position + 1 < self.history.len(),
                        Vec2::new(1.0, 0.0),
                    )
                    .on_hover_text("Forward · Alt+Right")
                    .clicked()
                    {
                        self.history_step(true, context);
                    }
                    if arrow_button(ui, self.current.parent().is_some(), Vec2::new(0.0, -1.0))
                        .on_hover_text("Parent · Alt+Up")
                        .clicked()
                    {
                        if let Some(parent) = self.current.parent() {
                            self.navigate_breadcrumb(parent.to_path_buf(), context);
                        }
                    }
                    if ui
                        .add_sized(
                            [48.0, 32.0],
                            egui::Button::new("Edit").selected(self.editing_path),
                        )
                        .on_hover_text("Toggle editable path / breadcrumbs · Ctrl+Shift+L")
                        .clicked()
                    {
                        self.search = None;
                        self.editing_path = !self.editing_path;
                        self.focus_path = self.editing_path;
                        self.address = self.current.to_string_lossy().into_owned();
                    }
                },
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(address)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(address));
                    self.address_bar(ui, context);
                },
            );
            ui.painter().line_segment(
                [
                    egui::pos2(options.left() - 8.0, row.top() + 6.0),
                    egui::pos2(options.left() - 8.0, row.bottom() - 6.0),
                ],
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(options)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    if theme_button(ui, self.settings.dark_mode).clicked() {
                        self.settings.dark_mode = !self.settings.dark_mode;
                        apply_theme(context, self.settings.dark_mode);
                        self.save();
                    }
                    if ui
                        .add_sized(
                            [64.0, 32.0],
                            egui::Button::new("Hidden").selected(self.settings.hidden),
                        )
                        .on_hover_text("Show hidden files · Ctrl+Shift+H")
                        .clicked()
                    {
                        self.settings.hidden = !self.settings.hidden;
                        self.refresh(context);
                        self.save();
                    }
                    if ui
                        .add_sized(
                            [92.0, 32.0],
                            egui::Button::new(if self.settings.sequences {
                                "Sequences"
                            } else {
                                "Images"
                            })
                            .selected(self.settings.sequences),
                        )
                        .on_hover_text("Toggle numbered image sequences / individual images")
                        .clicked()
                    {
                        self.settings.sequences = !self.settings.sequences;
                        self.refresh(context);
                        self.save();
                    }
                    if search_button(ui, self.search.is_some()).clicked() {
                        if self.search.is_some() {
                            self.search = None;
                        } else {
                            self.start_search(context);
                        }
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    for view in [View::Media, View::Grid, View::Tree] {
                        if view_button(ui, view, self.settings.view == view).clicked() {
                            self.settings.view = view;
                            self.save();
                        }
                    }
                },
            );
            ui.add_space(4.0);
        });
    }

    fn start_search(&mut self, context: &egui::Context) {
        self.sidebar_focused = false;
        self.editing_path = false;
        if let Some(search) = &mut self.search {
            search.focus = true;
        } else {
            self.search = Some(search::Search::new(
                self.current.clone(),
                self.settings.hidden,
                context.clone(),
            ));
        }
    }

    fn address_bar(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if let Some(search) = &mut self.search {
            let response = ui.add_sized(
                [ui.available_width(), 32.0],
                egui::TextEdit::singleline(&mut search.query)
                    .id(egui::Id::new("search-input"))
                    .vertical_align(egui::Align::Center)
                    .hint_text("Search this folder and subfolders…"),
            ).on_hover_text("Fuzzy search · 'exact · ^prefix · suffix$ · !exclude · term | term · Esc to close");
            if search.focus {
                response.request_focus();
                search.focus = false;
            }
            if response.has_focus() {
                self.sidebar_focused = false;
            }
            if response.changed() {
                search.submit();
            }
            let mut open = None;
            if let Some(update) = search.update.as_ref().filter(|_| !self.sidebar_focused) {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown)) {
                    search.cursor = (search.cursor + 1).min(update.hits.len().saturating_sub(1));
                    search.reveal_cursor = true;
                }
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp)) {
                    search.cursor = search.cursor.saturating_sub(1);
                    search.reveal_cursor = true;
                }
                if self.new_folder.is_none()
                    && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter))
                {
                    open = update.hits.get(search.cursor).map(|hit| hit.path.clone());
                }
            }
            if let Some(path) = open {
                self.open(path, context);
            }
        } else if self.editing_path {
            let response = ui.add_sized(
                [ui.available_width(), 32.0],
                egui::TextEdit::singleline(&mut self.address)
                    .vertical_align(egui::Align::Center)
                    .hint_text("Directory path"),
            );
            if self.focus_path {
                response.request_focus();
                if let Some(mut state) = egui::TextEdit::load_state(context, response.id) {
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(self.address.chars().count()),
                        )));
                    state.store(context, response.id);
                }
                self.focus_path = false;
            }
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(Key::Enter))
            {
                let path = if self.address == "~" {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                } else if let Some(tail) = self.address.strip_prefix("~/") {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(tail)
                } else {
                    PathBuf::from(&self.address)
                };
                let path = if path.is_absolute() {
                    path
                } else {
                    self.current.join(path)
                };
                self.navigate(path, context);
                response.surrender_focus();
            }
        } else {
            egui::ScrollArea::horizontal()
                .id_salt("breadcrumbs")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .stick_to_right(true)
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        let trail = self.breadcrumb_path.clone();
                        let mut path = PathBuf::new();
                        for component in trail.components() {
                            path.push(component);
                            let label = component.as_os_str().to_string_lossy();
                            if ui
                                .add(
                                    egui::Button::new(label.as_ref())
                                        .selected(path == self.current)
                                        .min_size(Vec2::new(0.0, 32.0)),
                                )
                                .clicked()
                            {
                                self.navigate_breadcrumb(path.clone(), context);
                            }
                        }
                    });
                });
        }
    }

    fn bookmark_row(
        &mut self,
        ui: &mut egui::Ui,
        bookmark: &bookmarks::Bookmark,
        kind: Option<places::Kind>,
        index: usize,
        context: &egui::Context,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
        paint_selection(
            ui,
            rect,
            self.sidebar_focused && self.sidebar_cursor == Some(index),
            false,
        );
        if self.sidebar_focused && self.sidebar_cursor == Some(index) && self.scroll_to_bookmark {
            ui.scroll_to_rect(rect, None);
            self.scroll_to_bookmark = false;
        }
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            Vec2::splat(18.0),
        );
        if let Some(kind) = kind {
            places::paint_icon(ui, icon, kind, ACCENT);
        } else {
            paint_folder(ui, icon);
        }
        paint_name(
            ui,
            &bookmark.label,
            egui::pos2(rect.left() + 34.0, rect.center().y),
            rect.width() - 40.0,
            13.0,
        );
        if response.clicked() {
            self.sidebar_focused = true;
            self.editing_path = false;
            context.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            self.sidebar_cursor = Some(index);
            self.open_bookmark(bookmark, context);
        }
        response.context_menu(|ui| {
            if ui.button("Remove bookmark").clicked() {
                if let Some(path) = bookmark.path.as_ref().filter(|_| kind.is_some()) {
                    if !self
                        .settings
                        .removed_places
                        .iter()
                        .any(|removed| removed == path)
                    {
                        self.settings.removed_places.push(path.to_path_buf());
                    }
                    self.save();
                } else if let Err(error) = self.bookmarks.remove(&bookmark.uri) {
                    self.message = Some(format!("Cannot save shared bookmarks: {error}"));
                }
                ui.close_menu();
            }
        });
    }

    fn open_bookmark(&mut self, bookmark: &bookmarks::Bookmark, context: &egui::Context) {
        if let Some(path) = &bookmark.path {
            self.navigate(path.clone(), context);
        } else if let Err(error) = std::process::Command::new("xdg-open")
            .arg(&bookmark.uri)
            .spawn()
        {
            self.message = Some(format!("Cannot open bookmark: {error}"));
        }
    }

    fn sidebar(&mut self, context: &egui::Context) {
        let mut entries: Vec<_> = self
            .home_places
            .iter()
            .filter(|place| !self.settings.removed_places.contains(&place.path))
            .map(|place| {
                (
                    bookmarks::Bookmark {
                        uri: String::new(),
                        path: Some(place.path.clone()),
                        label: place.label.into(),
                        exists: true,
                    },
                    Some(place.kind),
                )
            })
            .collect();
        let default_count = entries.len();
        entries.extend(
            self.bookmarks
                .entries
                .iter()
                .filter(|bookmark| {
                    bookmark.exists
                        && !self
                            .home_places
                            .iter()
                            .any(|place| bookmark.path.as_ref() == Some(&place.path))
                })
                .cloned()
                .map(|bookmark| (bookmark, None)),
        );
        if self.sidebar_focused && self.new_folder.is_none() && !self.text_input_focused(context) {
            let mut cursor = self
                .sidebar_cursor
                .unwrap_or_else(|| {
                    entries
                        .iter()
                        .position(|(bookmark, _)| bookmark.path.as_ref() == Some(&self.current))
                        .unwrap_or(0)
                })
                .min(entries.len().saturating_sub(1));
            let previous = cursor;
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                    || input.consume_key(egui::Modifiers::NONE, Key::J)
            }) {
                cursor = (cursor + 1).min(entries.len().saturating_sub(1));
            }
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                    || input.consume_key(egui::Modifiers::NONE, Key::K)
            }) {
                cursor = cursor.saturating_sub(1);
            }
            if let Some(index) = self.navigation_jump(entries.len(), context) {
                cursor = index;
            }
            let page_direction = context.input_mut(|input| {
                if input.consume_key(egui::Modifiers::CTRL, Key::D) {
                    1.0
                } else if input.consume_key(egui::Modifiers::CTRL, Key::U) {
                    -1.0
                } else {
                    0.0
                }
            });
            if page_direction != 0.0 {
                self.page_scroll_delta = page_direction * context.available_rect().height() / 2.0;
                let rows = (self.page_scroll_delta.abs()
                    / (32.0 + context.style().spacing.item_spacing.y))
                    .round()
                    .max(1.0) as usize;
                cursor = if page_direction > 0.0 {
                    cursor
                        .saturating_add(rows)
                        .min(entries.len().saturating_sub(1))
                } else {
                    cursor.saturating_sub(rows)
                };
                self.scroll_to_bookmark = false;
            } else {
                self.scroll_to_bookmark |= cursor != previous;
            }
            self.sidebar_cursor = Some(cursor);
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, Key::Enter)
                    || input.consume_key(egui::Modifiers::NONE, Key::Space)
                    || input.consume_key(egui::Modifiers::NONE, Key::O)
            }) {
                if let Some((bookmark, _)) = entries.get(cursor) {
                    self.open_bookmark(bookmark, context);
                }
            }
        }
        egui::SidePanel::left("bookmarks")
            .default_width(180.0)
            .width_range(140.0..=280.0)
            .show(context, |ui| {
                let drop = ui.interact(
                    ui.max_rect(),
                    ui.id().with("bookmark-drop"),
                    egui::Sense::hover(),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("BOOKMARKS").size(11.0).color(MUTED));
                    if ui
                        .small_button("+")
                        .on_hover_text("Bookmark selected or current folder · D")
                        .clicked()
                    {
                        self.bookmark();
                    }
                    ui.menu_button("...", |ui| {
                        if ui
                            .add_enabled(
                                !self.settings.removed_places.is_empty(),
                                egui::Button::new("Restore default folders"),
                            )
                            .clicked()
                        {
                            self.settings.removed_places.clear();
                            self.save();
                            ui.close_menu();
                        }
                    });
                });
                ui.add_space(12.0);
                self.entry_scroll_area(ui, "bookmark-list")
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        for (index, (bookmark, kind)) in entries.iter().enumerate() {
                            if index == default_count {
                                ui.separator();
                            }
                            self.bookmark_row(ui, bookmark, *kind, index, context);
                        }
                        ui.label(
                            RichText::new("Drag folders here\nor press D to bookmark")
                                .size(12.0)
                                .color(MUTED),
                        );
                    });
                if drop.dnd_hover_payload::<PathBuf>().is_some() {
                    ui.painter().rect_stroke(
                        ui.max_rect().shrink(2.0),
                        4.0,
                        Stroke::new(2.0, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(path) = drop.dnd_release_payload::<PathBuf>() {
                    self.add_bookmark((*path).clone());
                }
            });
    }

    fn statusbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(context, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} items", self.entries.len()))
                        .size(12.0)
                        .color(MUTED),
                );
                ui.separator();
                if self.visual_line && !self.sidebar_focused {
                    ui.label(
                        RichText::new("-- VISUAL LINE --")
                            .size(12.0)
                            .strong()
                            .color(visual_accent(ui.visuals().dark_mode)),
                    );
                    ui.separator();
                }
                if !self.sidebar_focused && self.selection.len() > 1 {
                    ui.label(
                        RichText::new(format!("{} selected", self.selection.len())).size(12.0),
                    );
                } else if !self.sidebar_focused && self.selected.is_some() {
                    if let Some(size) = &self.selection_size {
                        let suffix = if size.incomplete {
                            " (partial: unreadable items)"
                        } else {
                            ""
                        };
                        ui.label(
                            RichText::new(format!(
                                "Selected: {}{suffix}",
                                files::size_label(size.bytes)
                            ))
                            .size(12.0),
                        );
                    } else {
                        ui.label(RichText::new("Calculating size…").size(12.0).color(MUTED));
                    }
                } else if let Some(disk) = &self.disk {
                    ui.label(
                        RichText::new(format!(
                            "Disk: {} used / {} total",
                            files::size_label(disk.total.saturating_sub(disk.free)),
                            files::size_label(disk.total)
                        ))
                        .size(12.0),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui
                        .add(
                            egui::Slider::new(&mut self.settings.zoom, 0.65..=3.6)
                                .show_value(false),
                        )
                        .on_hover_text("Icon and tree zoom · Ctrl+wheel · up to 360%");
                    if response.changed() {
                        self.save();
                    }
                    ui.label(
                        RichText::new(format!("{}%", (self.settings.zoom * 100.0).round() as u32))
                            .size(12.0)
                            .color(MUTED),
                    );
                    if self.settings.view == View::Media {
                        ui.separator();
                        let response = ui
                            .add(
                                egui::Slider::new(&mut self.settings.media_margin, 0..=24)
                                    .show_value(false),
                            )
                            .on_hover_text("Media thumbnail margin · 0–24 px per side");
                        if response.changed() {
                            self.save();
                        }
                        ui.label(
                            RichText::new(format!("Margin: {} px", self.settings.media_margin))
                                .size(12.0)
                                .color(MUTED),
                        );
                    }
                    ui.separator();
                    if let Some(disk) = &self.disk {
                        ui.label(
                            RichText::new(format!("{} free", files::size_label(disk.available)))
                                .size(12.0)
                                .color(ACCENT),
                        );
                    }
                    if let Some((completed, total)) = self.thumbnails.progress(context) {
                        let label = if completed == total {
                            "Thumbnails ready"
                        } else {
                            "Caching thumbnails"
                        };
                        ui.add(
                            egui::ProgressBar::new(completed as f32 / total as f32)
                                .desired_width(ui.available_width().min(240.0))
                                .desired_height(16.0)
                                .text(
                                    RichText::new(format!("{label} · {completed}/{total}"))
                                        .size(12.0),
                                ),
                        );
                    }
                    if let Some(warning) = self.thumbnails.warning() {
                        ui.label(
                            RichText::new("Disk cache unavailable; using memory cache")
                                .size(11.0)
                                .color(MUTED),
                        )
                        .on_hover_text(warning);
                    }
                });
            });
            if self.compression_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Compressing to ZIP…");
                });
            }
            ui.add_space(3.0);
        });
    }

    fn entry_response(&mut self, response: egui::Response, entry: &Entry, context: &egui::Context) {
        if self
            .new_folder
            .as_ref()
            .is_some_and(|dialog| dialog.source.as_ref() == Some(&entry.path))
        {
            return;
        }
        if entry.directory {
            response.dnd_set_drag_payload(entry.path.clone());
        }
        if response.clicked() || response.secondary_clicked() {
            self.sidebar_focused = false;
        }
        if response.clicked()
            || (response.secondary_clicked() && !self.selection.contains(&entry.path))
        {
            self.select_entry(entry.path.clone(), context);
        }
        if response.double_clicked() && !context.input(|input| input.modifiers.shift) {
            self.open(entry.path.clone(), context);
        }
        let response = if let Some(error) = self
            .thumbnails
            .entry_error(entry)
            .filter(|_| self.settings.view != View::Media || entry.directory)
        {
            response.on_hover_text(format!("Thumbnail unavailable: {error}"))
        } else {
            response
        };
        response.context_menu(|ui| {
            if ui
                .button(if entry.sequence.is_some() {
                    "Open first frame"
                } else {
                    "Open"
                })
                .clicked()
            {
                self.open(entry.path.clone(), context);
                ui.close_menu();
            }
            if entry.directory {
                if ui.button("Open Konsole here").clicked() {
                    self.open_terminal(&entry.path);
                    ui.close_menu();
                }
                if ui.button("Add bookmark · D").clicked() {
                    self.add_bookmark(entry.path.clone());
                    ui.close_menu();
                }
            }
            if entry.sequence.is_none()
                && (entry.directory || files::image_file(&entry.path))
                && ui
                    .add_enabled(
                        self.compression_job.is_none(),
                        egui::Button::new("Compress to ZIP"),
                    )
                    .clicked()
            {
                self.compress(entry.path.clone(), context);
                ui.close_menu();
            }
            if ui.button("Copy path").clicked() {
                context.copy_text(entry.path.to_string_lossy().into_owned());
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Move to Trash · Delete").clicked() {
                self.trash_selection(context);
                ui.close_menu();
            }
            if ui.button("Properties").clicked() {
                self.show_properties(&entry.path);
                ui.close_menu();
            }
        });
    }

    fn paint_thumbnail(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        entry: &Entry,
        response: &egui::Response,
    ) -> bool {
        let video = entry.animated();
        if entry.directory
            || (!files::image_file(&entry.path) && !video)
            || !ui.is_rect_visible(rect)
        {
            return false;
        }
        let progress = if video && response.hovered() {
            response.hover_pos().map(|pointer| {
                ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0)
            })
        } else {
            None
        };
        let poster = if video { images::VIDEO_POSTER } else { 0 };
        let frame = progress.map_or(poster, |progress| {
            ((progress * images::VIDEO_FRAMES as f32) as usize).min(images::VIDEO_FRAMES - 1)
        });
        let Some(texture) = self.thumbnails.get_entry(entry, frame) else {
            return false;
        };
        let dimensions = texture.size_vec2();
        let image_bounds = if self.settings.view == View::Media {
            rect.shrink(
                (self.settings.media_margin as f32).min(rect.width().min(rect.height()) / 4.0),
            )
        } else {
            rect
        };
        let scale = (image_bounds.width() / dimensions.x).min(image_bounds.height() / dimensions.y);
        let image_rect = egui::Rect::from_center_size(image_bounds.center(), dimensions * scale);
        let uv_size = image_rect.size() / (dimensions * scale);
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_center_size(egui::pos2(0.5, 0.5), uv_size),
            Color32::WHITE,
        );
        if progress.is_some() {
            let displayed = self
                .thumbnails
                .displayed_frame(entry, frame)
                .unwrap_or(poster);
            let progress = if let Some(paths) = &entry.sequence {
                let index = if displayed == images::VIDEO_POSTER {
                    paths.len() / 2
                } else {
                    (paths.len() - 1) * displayed / (images::VIDEO_FRAMES - 1)
                };
                index as f32 / (paths.len() - 1) as f32
            } else if displayed == images::VIDEO_POSTER {
                0.5
            } else {
                displayed as f32 / (images::VIDEO_FRAMES - 1) as f32
            };
            let inset = image_bounds.shrink(4.0);
            let height = PREVIEW_BAR_HEIGHT.min(inset.height()).max(0.0);
            let left = egui::pos2(inset.left(), inset.bottom() - height);
            let width = inset.width().max(0.0);
            let track = egui::Rect::from_min_size(left, Vec2::new(width, height));
            let background = if ui.visuals().dark_mode {
                Color32::from_rgb(55, 62, 70)
            } else {
                Color32::from_rgb(208, 214, 220)
            };
            ui.painter().rect_filled(track, 2.0, background);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(left, Vec2::new(width * progress, height)),
                2.0,
                ACCENT,
            );
        }
        true
    }

    fn search_results(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let mut open = None;
        let mut select = None;
        if let Some(search) = &mut self.search {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if search.busy {
                    ui.spinner();
                }
                if let Some(update) = &search.update {
                    ui.label(format!(
                        "{} matches · {} indexed",
                        update.matched, update.indexed
                    ));
                    if update.matched > update.hits.len() {
                        ui.label(format!("Showing the best {}", update.hits.len()));
                    }
                    if update.skipped > 0 {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("{} unreadable items skipped", update.skipped),
                        );
                    }
                } else {
                    ui.label("Searching…");
                }
            });
            ui.separator();
            if let Some(update) = &search.update {
                if let Some(error) = &update.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if update.hits.is_empty() && !search.busy {
                    ui.label("No matches");
                }
                egui::ScrollArea::vertical()
                    .id_salt("search-results")
                    .auto_shrink([false, false])
                    .show_rows(ui, 32.0, update.hits.len(), |ui, range| {
                        if search.reveal_cursor && !update.hits.is_empty() {
                            let height = 32.0 + ui.spacing().item_spacing.y;
                            let top = ui.max_rect().top()
                                + (search.cursor as f32 - range.start as f32) * height;
                            ui.scroll_to_rect(
                                egui::Rect::from_min_size(
                                    egui::pos2(ui.max_rect().left(), top),
                                    Vec2::new(ui.available_width(), 32.0),
                                ),
                                None,
                            );
                            search.reveal_cursor = false;
                        }
                        for index in range {
                            let hit = &update.hits[index];
                            let label = if hit.directory {
                                format!("{}/", hit.label)
                            } else {
                                hit.label.clone()
                            };
                            let response = ui.add_sized(
                                [ui.available_width(), 32.0],
                                egui::Button::new(label)
                                    .selected(!self.sidebar_focused && search.cursor == index),
                            );
                            if response.clicked() {
                                search.cursor = index;
                                select = Some(hit.path.clone());
                            }
                            if response.double_clicked() {
                                open = Some(hit.path.clone());
                            }
                        }
                    });
            }
        }
        if let Some(path) = select {
            self.select(Some(path), context);
        }
        if let Some(path) = open {
            self.open(path, context);
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let entries = self.entries.clone();
        let width = 116.0 * self.settings.zoom;
        let height = 126.0 * self.settings.zoom;
        let available = ui.available_width() - ui.spacing().scroll.allocated_width();
        let columns = ((available - 16.0 + 8.0) / (width + 8.0)).floor().max(1.0) as usize;
        let rows = entries.len().div_ceil(columns);
        let items: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                (
                    entry.clone(),
                    egui::pos2(
                        (index % columns) as f32 * (width + ui.spacing().item_spacing.x),
                        (index / columns) as f32 * (height + ui.spacing().item_spacing.y),
                    ),
                )
            })
            .collect();
        self.navigate_items(&items, ui.available_height(), context);
        ui.scope(|ui| {
            let mut scroll = self.entry_scroll_area(ui, "grid");
            if self.scroll_to_selection {
                if let Some(index) = entries
                    .iter()
                    .position(|entry| self.selected.as_ref() == Some(&entry.path))
                {
                    let top = (index / columns) as f32 * (height + ui.spacing().item_spacing.y);
                    let viewport = ui.available_height();
                    let offset =
                        egui::scroll_area::State::load(context, ui.make_persistent_id("grid"))
                            .map_or(0.0, |state| state.offset.y);
                    scroll = scroll.vertical_scroll_offset(
                        offset.min(top).max(top + height - viewport).max(0.0),
                    );
                }
                self.scroll_to_selection = false;
            }
            scroll
                .auto_shrink([false, false])
                .id_salt("grid")
                .drag_to_scroll(false)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show_rows(ui, height, rows, |ui, range| {
                    let start = (range.start * columns).min(entries.len());
                    let end = (range.end * columns).min(entries.len());
                    for row in entries[start..end].chunks(columns) {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            for entry in row {
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(width, height),
                                    if entry.directory {
                                        egui::Sense::click_and_drag()
                                    } else {
                                        egui::Sense::click()
                                    },
                                );
                                if !self.sidebar_focused && self.selection.contains(&entry.path) {
                                    ui.painter().rect_filled(
                                        rect.shrink(2.0),
                                        8.0,
                                        ui.visuals().selection.bg_fill,
                                    );
                                }
                                let icon_rect = egui::Rect::from_center_size(
                                    rect.center_top() + Vec2::new(0.0, height * 0.34),
                                    Vec2::splat(64.0 * self.settings.zoom),
                                );
                                if !self.paint_thumbnail(ui, icon_rect, entry, &response) {
                                    paint_icon(
                                        ui,
                                        icon_rect.shrink(8.0 * self.settings.zoom),
                                        entry,
                                    );
                                }
                                let name_rect = egui::Rect::from_min_size(
                                    egui::pos2(rect.left() + 7.0, rect.top() + height * 0.66),
                                    Vec2::new(width - 14.0, 24.0),
                                );
                                if !self.rename_field(ui, name_rect, entry, context) {
                                    let mut job = egui::text::LayoutJob::simple(
                                        entry.name.clone(),
                                        egui::FontId::proportional(12.0),
                                        ui.visuals().text_color(),
                                        width - 14.0,
                                    );
                                    job.wrap.max_rows = 2;
                                    job.halign = egui::Align::Center;
                                    let galley = ui.painter().layout_job(job);
                                    ui.painter().galley(
                                        egui::pos2(rect.center().x, rect.top() + height * 0.66),
                                        galley,
                                        ui.visuals().text_color(),
                                    );
                                }
                                self.entry_response(response, entry, context);
                            }
                        });
                    }
                });
        });
    }

    fn media(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| {
                entry.directory || files::image_file(&entry.path) || files::video_file(&entry.path)
            })
            .cloned()
            .collect();
        if entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No folders, images or videos in this folder").color(MUTED));
            });
            return;
        }
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(1.0);
        let aspects: Vec<_> = entries
            .iter()
            .map(|entry| {
                if entry.directory {
                    1.0
                } else {
                    self.thumbnails.entry_aspect_ratio(entry)
                }
            })
            .collect();
        let rows = layout::justified_rows(&aspects, width, 192.0 * self.settings.zoom);
        let total_height: f32 = rows.iter().map(|(_, height)| height).sum();
        let mut items = Vec::with_capacity(entries.len());
        let mut y = 0.0;
        for (range, height) in &rows {
            let mut x = 0.0;
            for index in range.clone() {
                let item_width = aspects[index] * height;
                items.push((entries[index].clone(), egui::pos2(x + item_width / 2.0, y)));
                x += item_width;
            }
            y += height;
        }
        self.navigate_items(&items, ui.available_height(), context);
        self.entry_scroll_area(ui, "media")
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_viewport(ui, |ui, _| {
                ui.set_min_size(Vec2::new(width, total_height));
                let origin = ui.min_rect().min;
                let mut y = origin.y;
                for (range, height) in rows {
                    let mut x = origin.x;
                    for index in range {
                        let entry = &entries[index];
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(x, y),
                            Vec2::new(aspects[index] * height, height),
                        );
                        x = rect.right();
                        if self.scroll_to_selection && self.selected.as_ref() == Some(&entry.path) {
                            ui.scroll_to_rect(rect, None);
                            self.scroll_to_selection = false;
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let response = ui.interact(
                            rect,
                            ui.id().with(&entry.path),
                            if entry.directory {
                                egui::Sense::click_and_drag()
                            } else {
                                egui::Sense::click()
                            },
                        );
                        let highlighted =
                            !self.sidebar_focused && self.selection.contains(&entry.path);
                        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
                        if !self.paint_thumbnail(ui, rect, entry, &response) {
                            let edge = (64.0 * self.settings.zoom)
                                .min(rect.width().min(rect.height()) * 0.5);
                            paint_icon(
                                ui,
                                egui::Rect::from_center_size(rect.center(), Vec2::splat(edge)),
                                entry,
                            );
                        }
                        if entry.directory {
                            let label = egui::Rect::from_min_max(
                                egui::pos2(rect.left(), (rect.bottom() - 26.0).max(rect.top())),
                                rect.max,
                            );
                            ui.painter()
                                .rect_filled(label, 0.0, ui.visuals().panel_fill);
                            let mut job = egui::text::LayoutJob::simple(
                                entry.name.clone(),
                                egui::FontId::proportional(12.0),
                                ui.visuals().text_color(),
                                (label.width() - 12.0).max(1.0),
                            );
                            job.wrap.max_rows = 1;
                            job.halign = egui::Align::Center;
                            let galley = ui.painter().layout_job(job);
                            let position = egui::pos2(
                                label.center().x,
                                label.center().y - galley.size().y / 2.0,
                            );
                            ui.painter()
                                .with_clip_rect(ui.clip_rect().intersect(label))
                                .galley(position, galley, ui.visuals().text_color());
                        }
                        if highlighted {
                            ui.painter().rect_stroke(
                                rect.shrink(1.0),
                                0.0,
                                Stroke::new(2.0, ui.visuals().selection.stroke.color),
                                egui::StrokeKind::Inside,
                            );
                        }
                        let name_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 4.0, (rect.bottom() - 26.0).max(rect.top())),
                            egui::pos2(rect.right() - 4.0, rect.bottom() - 2.0),
                        );
                        self.rename_field(ui, name_rect, entry, context);
                        self.entry_response(response, entry, context);
                    }
                    y += height;
                }
            });
    }

    fn tree_header(&mut self, ui: &mut egui::Ui) {
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(1.0);
        let (row, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), egui::Sense::hover());
        let date_left = row.right() - TREE_DATE_WIDTH;
        let size_left = date_left - TREE_SIZE_WIDTH;
        for (label, column, left, right, x, align) in [
            (
                "Name",
                TreeSort::Name,
                row.left(),
                size_left,
                row.left() + 36.0,
                egui::Align2::LEFT_CENTER,
            ),
            (
                "Size",
                TreeSort::Size,
                size_left,
                date_left,
                date_left - 12.0,
                egui::Align2::RIGHT_CENTER,
            ),
            (
                "Date Modified",
                TreeSort::Modified,
                date_left,
                row.right(),
                date_left + 8.0,
                egui::Align2::LEFT_CENTER,
            ),
        ] {
            let rect = egui::Rect::from_min_max(
                egui::pos2(left, row.top()),
                egui::pos2(right, row.bottom()),
            );
            let response = ui.interact(
                rect,
                ui.id().with(("tree-sort", label)),
                egui::Sense::click(),
            );
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if response.clicked() {
                if self.settings.tree_sort == column {
                    self.settings.tree_descending = !self.settings.tree_descending;
                } else {
                    self.settings.tree_sort = column;
                    self.settings.tree_descending = false;
                }
                self.save();
            }
            let active = self.settings.tree_sort == column;
            let text_x = if active && align == egui::Align2::RIGHT_CENTER {
                x - 16.0
            } else {
                x
            };
            let text_rect = ui.painter().text(
                egui::pos2(text_x, row.center().y),
                align,
                label,
                egui::FontId::proportional(12.0),
                if active { ACCENT } else { MUTED },
            );
            if active {
                paint_arrow(
                    ui,
                    egui::Rect::from_center_size(
                        egui::pos2(text_rect.right() + 10.0, row.center().y),
                        Vec2::splat(14.0),
                    ),
                    ACCENT,
                    Vec2::new(
                        0.0,
                        if self.settings.tree_descending {
                            1.0
                        } else {
                            -1.0
                        },
                    ),
                );
            }
            response.on_hover_text(match column {
                TreeSort::Name => "Sort by name; click again to reverse",
                TreeSort::Size => "Sort by folder item count or file bytes; click again to reverse",
                TreeSort::Modified => {
                    "Sort by modification date (local time); click again to reverse"
                }
            });
        }
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        for x in [date_left - TREE_SIZE_WIDTH, date_left] {
            ui.painter().line_segment(
                [
                    egui::pos2(x, row.top() + 6.0),
                    egui::pos2(x, row.bottom() - 6.0),
                ],
                stroke,
            );
        }
        ui.painter()
            .line_segment([row.left_bottom(), row.right_bottom()], stroke);
    }

    fn sort_tree_entries(&self, entries: &mut [Entry]) {
        entries.sort_by(|a, b| {
            let name_order = || {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.name.cmp(&b.name))
            };
            let order = match self.settings.tree_sort {
                TreeSort::Name => name_order(),
                TreeSort::Size => {
                    let size = |entry: &Entry| {
                        if entry.directory {
                            entry.item_count.map(|count| count as u64)
                        } else {
                            Some(entry.size)
                        }
                    };
                    size(a).cmp(&size(b)).then_with(name_order)
                }
                TreeSort::Modified => a.modified.cmp(&b.modified).then_with(name_order),
            };
            b.directory
                .cmp(&a.directory)
                .then(if self.settings.tree_descending {
                    order.reverse()
                } else {
                    order
                })
        });
    }

    fn tree_items(
        &self,
        mut entries: Vec<Entry>,
        depth: usize,
        items: &mut Vec<(Entry, egui::Pos2)>,
    ) {
        self.sort_tree_entries(&mut entries);
        for entry in entries {
            let path = entry.path.clone();
            let y = items.len() as f32 * (30.0 * self.settings.zoom).max(32.0);
            items.push((entry, egui::pos2(0.0, y)));
            if self.expanded.contains(&path) && depth < 64 {
                if let Some(children) = self.tree_cache.get(&path) {
                    self.tree_items(children.clone(), depth + 1, items);
                }
            }
        }
    }

    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        mut entries: Vec<Entry>,
        depth: usize,
        row_index: &mut usize,
        context: &egui::Context,
    ) {
        self.sort_tree_entries(&mut entries);
        for entry in entries {
            ui.push_id(&entry.path, |ui| {
                let height = (30.0 * self.settings.zoom).max(32.0);
                let (row, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                if self.scroll_to_selection && self.selected.as_ref() == Some(&entry.path) {
                    ui.scroll_to_rect(row, None);
                    self.scroll_to_selection = false;
                }
                ui.painter().rect_filled(
                    row,
                    0.0,
                    if *row_index % 2 == 0 {
                        ui.visuals().panel_fill
                    } else {
                        ui.visuals().faint_bg_color
                    },
                );
                *row_index += 1;
                let left = row.left() + depth as f32 * 20.0 * self.settings.zoom;
                let disclosure = egui::Rect::from_center_size(
                    egui::pos2(left + 14.0, row.center().y),
                    Vec2::splat(24.0),
                );
                if entry.directory && !entry.symlink {
                    let response =
                        ui.interact(disclosure, ui.id().with("expand"), egui::Sense::click());
                    let expanded = self.expanded.contains(&entry.path);
                    paint_arrow(
                        ui,
                        disclosure,
                        ui.visuals().text_color(),
                        if expanded {
                            Vec2::new(0.0, 1.0)
                        } else {
                            Vec2::new(1.0, 0.0)
                        },
                    );
                    if response.clicked() {
                        if expanded {
                            self.expanded.remove(&entry.path);
                        } else {
                            self.expand_directory(&entry.path);
                        }
                    }
                }
                let content = egui::Rect::from_min_max(egui::pos2(left + 30.0, row.top()), row.max);
                let response = ui.interact(
                    content,
                    ui.id().with("entry"),
                    if entry.directory {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::click()
                    },
                );
                paint_selection(
                    ui,
                    content,
                    !self.sidebar_focused && self.selection.contains(&entry.path),
                    false,
                );
                let edge = 22.0 * self.settings.zoom;
                let icon = egui::Rect::from_center_size(
                    egui::pos2(content.left() + edge / 2.0 + 6.0, row.center().y),
                    Vec2::splat(edge),
                );
                if !self.paint_thumbnail(ui, icon, &entry, &response) {
                    paint_icon(ui, icon, &entry);
                }
                let name_left = icon.right() + 10.0;
                let date_left = row.right() - TREE_DATE_WIDTH;
                let name_right = date_left - TREE_SIZE_WIDTH;
                let name_rect = egui::Rect::from_min_size(
                    egui::pos2(name_left, row.top() + 4.0),
                    Vec2::new((name_right - name_left - 8.0).max(1.0), row.height() - 8.0),
                );
                if !self.rename_field(ui, name_rect, &entry, context) {
                    paint_name(
                        ui,
                        &entry.name,
                        egui::pos2(name_left, row.center().y),
                        name_rect.width(),
                        14.0,
                    );
                }
                let size = if entry.directory {
                    entry.item_count.map_or_else(
                        || "—".to_owned(),
                        |count| format!("{count} {}", if count == 1 { "item" } else { "items" }),
                    )
                } else {
                    files::size_label(entry.size)
                };
                ui.painter().text(
                    egui::pos2(date_left - 12.0, row.center().y),
                    egui::Align2::RIGHT_CENTER,
                    size,
                    egui::FontId::proportional(11.0),
                    MUTED,
                );
                ui.painter().text(
                    egui::pos2(date_left + 8.0, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    &entry.modified_label,
                    egui::FontId::proportional(11.0),
                    MUTED,
                );
                self.entry_response(response, &entry, context);
                if self.expanded.contains(&entry.path) && depth < 64 {
                    if let Some(children) = self.tree_cache.get(&entry.path).cloned() {
                        self.tree(ui, children, depth + 1, row_index, context);
                    }
                }
            });
        }
    }

    fn trash_selection(&mut self, context: &egui::Context) {
        let mut paths = Vec::new();
        for path in &self.selection {
            let sequence = self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .find(|entry| &entry.path == path)
                .and_then(|entry| entry.sequence.as_ref());
            if let Some(sequence) = sequence {
                paths.extend(sequence.iter().cloned());
            } else {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        let roots = paths
            .iter()
            .filter(|path| {
                !paths
                    .iter()
                    .any(|parent| parent != *path && path.starts_with(parent))
            })
            .cloned()
            .collect();
        let outcome = self.operations.trash(roots);
        self.complete_operation(outcome, context);
    }

    fn complete_operation(&mut self, outcome: operations::Outcome, context: &egui::Context) {
        if outcome.changed {
            let nearby = self.selected.as_ref().and_then(|selected| {
                let index = self
                    .visible_paths
                    .iter()
                    .position(|path| path == selected)?;
                self.visible_paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| std::fs::symlink_metadata(path).is_ok())
                    .min_by_key(|(position, _)| position.abs_diff(index))
                    .map(|(_, path)| path.clone())
            });
            self.message = None;
            for (from, to) in &outcome.renames {
                let relocate = |path: &Path| {
                    path.strip_prefix(from)
                        .map_or_else(|_| path.to_path_buf(), |suffix| to.join(suffix))
                };
                self.current = relocate(&self.current);
                self.breadcrumb_path = relocate(&self.breadcrumb_path);
                for path in &mut self.history {
                    *path = relocate(path);
                }
                self.expanded = self.expanded.iter().map(|path| relocate(path)).collect();
            }
            if !self.current.is_dir() {
                if let Some(parent) = self.current.ancestors().find(|path| path.is_dir()) {
                    self.navigate(parent.to_path_buf(), context);
                }
            } else {
                self.address = self.current.to_string_lossy().into_owned();
                self.refresh(context);
            }
            for expanded in self.expanded.clone() {
                if expanded.is_dir() {
                    self.expand_directory(&expanded);
                }
            }
            let selected = outcome
                .selected
                .or(nearby)
                .filter(|path| {
                    path.parent() == Some(self.current.as_path())
                        || (self.settings.view == View::Tree
                            && path != &self.current
                            && path.starts_with(&self.current))
                })
                .or_else(|| self.entries.first().map(|entry| entry.path.clone()));
            if self.settings.view == View::Tree {
                if let Some(path) = &selected {
                    let parents: Vec<_> = path
                        .ancestors()
                        .skip(1)
                        .take_while(|parent| *parent != self.current)
                        .map(Path::to_path_buf)
                        .collect();
                    for parent in parents.into_iter().rev() {
                        self.expand_directory(&parent);
                    }
                }
            }
            self.select(selected, context);
            self.scroll_to_selection = true;
        }
        if outcome.error.is_some() {
            self.message = outcome.error;
        }
    }

    fn start_rename(&mut self) {
        let Some(source) = self.selected.clone() else {
            return;
        };
        if self.selection.len() > 1 {
            self.message = Some("Select a single item to rename.".into());
            return;
        }
        if self
            .entries
            .iter()
            .chain(self.tree_cache.values().flatten())
            .any(|entry| entry.path == source && entry.sequence.is_some())
        {
            self.message = Some("Disable sequence grouping before renaming a frame.".into());
            return;
        }
        let (Some(parent), Some(name)) = (source.parent(), source.file_name()) else {
            return;
        };
        let Some(name) = name.to_str() else {
            self.message = Some("Cannot edit a name that is not valid UTF-8.".into());
            return;
        };
        self.new_folder = Some(NameDialog {
            parent: parent.to_path_buf(),
            name: name.to_owned(),
            source: Some(source),
            submit: false,
            focus: true,
            error: None,
        });
    }

    fn rename_field(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        entry: &Entry,
        context: &egui::Context,
    ) -> bool {
        let Some(dialog) = self
            .new_folder
            .as_mut()
            .filter(|dialog| dialog.source.as_ref() == Some(&entry.path))
        else {
            return false;
        };
        let id = ui.make_persistent_id(("rename-entry", &entry.path));
        let response = ui.put(rect, egui::TextEdit::singleline(&mut dialog.name).id(id));
        if dialog.focus {
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(context, id) {
                let length = if entry.directory {
                    dialog.name.chars().count()
                } else {
                    entry
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .map_or(dialog.name.chars().count(), |stem| stem.chars().count())
                };
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(length),
                    )));
                state.store(context, id);
            }
            dialog.focus = false;
        }
        dialog.submit = (response.has_focus() || response.lost_focus())
            && ui.input(|input| input.key_pressed(Key::Enter));
        if response.lost_focus() && !dialog.submit {
            self.new_folder = None;
        }
        true
    }

    fn new_folder_window(&mut self, context: &egui::Context) {
        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        if let Some(folder) = &mut self.new_folder {
            if folder.source.is_some() {
                create = folder.submit;
            } else {
                egui::Window::new("New folder")
                    .id(egui::Id::new("new-folder"))
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .show(context, |ui| {
                        ui.label(folder.parent.to_string_lossy());
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut folder.name).hint_text("Folder name"),
                        );
                        if folder.focus {
                            response.request_focus();
                            folder.focus = false;
                        }
                        create = (response.has_focus() || response.lost_focus())
                            && ui.input(|input| input.key_pressed(Key::Enter));
                        if let Some(error) = &folder.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        ui.horizontal(|ui| {
                            create |= ui
                                .add_enabled(
                                    !folder.name.trim().is_empty(),
                                    egui::Button::new("Create"),
                                )
                                .clicked();
                            cancel = ui.button("Cancel").clicked();
                        });
                    });
            }
        }
        if !open || cancel {
            self.new_folder = None;
        } else if create {
            if let Some(folder) = &mut self.new_folder {
                let source = folder.source.clone();
                let result = if let Some(source) = &source {
                    files::rename_entry(source, &folder.name)
                } else {
                    files::create_directory(&folder.parent, &folder.name)
                };
                match result {
                    Ok(path) => {
                        if self.message == folder.error {
                            self.message = None;
                        }
                        let mut renames = Vec::new();
                        if let Some(source) = source {
                            self.operations.record_rename(source.clone(), path.clone());
                            renames.push((source, path.clone()));
                        } else {
                            self.operations.discard_redo();
                        }
                        self.new_folder = None;
                        self.complete_operation(
                            operations::Outcome {
                                changed: true,
                                renames,
                                selected: Some(path),
                                error: None,
                            },
                            context,
                        );
                    }
                    Err(error) => {
                        let error = format!("Cannot save name: {error}");
                        if folder.source.is_some() {
                            self.message = Some(error.clone());
                            folder.focus = true;
                            folder.submit = false;
                        }
                        folder.error = Some(error);
                    }
                }
            }
        }
    }
}

impl eframe::App for FileManager {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        context.input_mut(|input| {
            for event in &mut input.events {
                if let egui::Event::Key {
                    key,
                    physical_key,
                    modifiers,
                    ..
                } = event
                {
                    if modifiers.matches_logically(egui::Modifiers::CTRL) {
                        let replacement = match *key {
                            Key::M => Some(Key::Enter),
                            Key::OpenBracket => Some(Key::Escape),
                            _ => None,
                        };
                        if let Some(replacement) = replacement {
                            *key = replacement;
                            *physical_key = None;
                            *modifiers = egui::Modifiers::NONE;
                        }
                    }
                }
            }
        });
        context.request_repaint_after(std::time::Duration::from_secs(1));
        if let Err(error) = self.bookmarks.poll() {
            self.message = Some(format!("Cannot read shared bookmarks: {error}"));
        }
        self.poll(context);
        self.shortcuts(context);
        self.toolbar(context);
        self.statusbar(context);
        self.sidebar(context);
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&context.style()).inner_margin(0))
            .show(context, |ui| {
                if self.visual_line {
                    let visuals = ui.visuals_mut();
                    visuals.selection.bg_fill = if visuals.dark_mode {
                        Color32::from_rgb(79, 45, 108)
                    } else {
                        Color32::from_rgb(224, 199, 245)
                    };
                    visuals.selection.stroke = Stroke::new(1.0, visual_accent(visuals.dark_mode));
                }
                let background = ui.interact(
                    ui.available_rect_before_wrap(),
                    ui.id().with("directory-background"),
                    egui::Sense::click(),
                );
                if let Some(message) = self.message.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().warn_fg_color, message);
                        if ui.small_button("Dismiss").clicked() {
                            self.message = None;
                        }
                    });
                    ui.separator();
                }
                if self.search.is_some() {
                    self.search_results(ui, context);
                } else if self.entries.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("This folder is empty").color(MUTED));
                    });
                } else {
                    match self.settings.view {
                        View::Grid => self.grid(ui, context),
                        View::Media => self.media(ui, context),
                        View::Tree => {
                            self.tree_header(ui);
                            let mut items = Vec::new();
                            self.tree_items(self.entries.clone(), 0, &mut items);
                            self.navigate_items(&items, ui.available_height(), context);
                            self.entry_scroll_area(ui, "tree")
                                .auto_shrink([false, false])
                                .drag_to_scroll(false)
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                )
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    self.tree(ui, self.entries.clone(), 0, &mut 0, context);
                                });
                        }
                    }
                }
                if background.clicked() || background.secondary_clicked() {
                    self.sidebar_focused = false;
                    self.select(None, context);
                }
                background.context_menu(|ui| self.directory_menu(ui, context));
            });
        self.new_folder_window(context);
        if let Some(path) = egui::DragAndDrop::payload::<PathBuf>(context) {
            context.set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(pointer) = context.pointer_hover_pos() {
                egui::Area::new(egui::Id::new("dragged-folder"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(pointer + Vec2::new(12.0, 12.0))
                    .interactable(false)
                    .show(context, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                        });
                    });
            }
        }
    }
}

fn view_button(ui: &mut egui::Ui, view: View, selected: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").selected(selected));
    let icon = egui::Rect::from_center_size(response.rect.center(), Vec2::splat(16.0));
    let stroke = Stroke::new(1.5, ui.visuals().text_color());
    let point = |x, y| icon.min + icon.size() * Vec2::new(x, y);
    match view {
        View::Tree => {
            ui.painter()
                .line_segment([point(0.1, 0.1), point(0.1, 0.9)], stroke);
            for y in [0.1, 0.5, 0.9] {
                ui.painter()
                    .line_segment([point(0.1, y), point(0.9, y)], stroke);
            }
        }
        View::Grid => {
            for x in [0.0, 0.6] {
                for y in [0.0, 0.6] {
                    ui.painter().rect_stroke(
                        egui::Rect::from_min_max(point(x, y), point(x + 0.4, y + 0.4)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        View::Media => {
            for (left, top, right, bottom) in [
                (0.0, 0.0, 0.6, 0.4),
                (0.7, 0.0, 1.0, 0.4),
                (0.0, 0.5, 0.3, 1.0),
                (0.4, 0.5, 1.0, 1.0),
            ] {
                ui.painter().rect_stroke(
                    egui::Rect::from_min_max(point(left, top), point(right, bottom)),
                    0.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
    response.on_hover_text(match view {
        View::Tree => "Tree",
        View::Grid => "Grid",
        View::Media => "Media",
    })
}

fn search_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").selected(selected));
    let center = response.rect.center() - Vec2::splat(2.0);
    let stroke = Stroke::new(1.7, ui.visuals().text_color());
    ui.painter().circle_stroke(center, 6.0, stroke);
    ui.painter().line_segment(
        [center + Vec2::splat(4.5), center + Vec2::splat(10.0)],
        stroke,
    );
    response.on_hover_text("Search · Ctrl+P / Ctrl+F")
}

fn theme_button(ui: &mut egui::Ui, dark_mode: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new(""));
    let center = response.rect.center();
    let radius = 8.0;
    ui.painter().circle_filled(center, radius, Color32::WHITE);
    let mut half = vec![center];
    for step in 0..=16 {
        let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * step as f32 / 16.0;
        half.push(center + Vec2::new(angle.cos(), angle.sin()) * radius);
    }
    ui.painter().add(egui::Shape::convex_polygon(
        half,
        Color32::BLACK,
        Stroke::NONE,
    ));
    ui.painter()
        .circle_stroke(center, radius, Stroke::new(1.0, ui.visuals().text_color()));
    response.on_hover_text(if dark_mode {
        "Switch to light theme"
    } else {
        "Switch to dark theme"
    })
}

fn arrow_button(ui: &mut egui::Ui, enabled: bool, direction: Vec2) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("").min_size(Vec2::new(32.0, 32.0)),
    );
    let color = if enabled {
        ui.visuals().text_color()
    } else {
        MUTED.gamma_multiply(0.4)
    };
    paint_arrow(ui, response.rect, color, direction);
    response
}

fn paint_arrow(ui: &egui::Ui, rect: egui::Rect, color: Color32, direction: Vec2) {
    let center = rect.center();
    let tip = center + direction * 6.0;
    let side = Vec2::new(-direction.y, direction.x) * 4.0;
    let stroke = Stroke::new(1.5, color);
    for segment in [
        [center - direction * 6.0, tip],
        [tip - direction * 4.0 + side, tip],
        [tip - direction * 4.0 - side, tip],
    ] {
        ui.painter().line_segment(segment, stroke);
    }
}

fn paint_selection(ui: &egui::Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    if selected || hovered {
        ui.painter().rect_filled(
            rect.shrink(2.0),
            5.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.hovered.weak_bg_fill
            },
        );
    }
}

fn paint_name(ui: &egui::Ui, name: &str, center: egui::Pos2, width: f32, size: f32) {
    let mut job = egui::text::LayoutJob::simple(
        name.into(),
        egui::FontId::proportional(size),
        ui.visuals().text_color(),
        width.max(1.0),
    );
    job.wrap.max_rows = 1;
    let galley = ui.painter().layout_job(job);
    let position = egui::pos2(center.x, center.y - galley.size().y / 2.0);
    ui.painter()
        .galley(position, galley, ui.visuals().text_color());
}

fn paint_folder(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            rect.min + Vec2::new(0.0, rect.height() * 0.15),
            rect.min + Vec2::new(rect.width() * 0.46, rect.height() * 0.4),
        ),
        3.0,
        ACCENT,
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            rect.min + Vec2::new(0.0, rect.height() * 0.3),
            rect.max - Vec2::new(0.0, rect.height() * 0.1),
        ),
        4.0,
        ACCENT,
    );
}

fn paint_icon(ui: &egui::Ui, rect: egui::Rect, entry: &Entry) {
    let painter = ui.painter();
    if entry.directory {
        paint_folder(ui, rect);
    } else {
        let color = if files::image_file(&entry.path) {
            Color32::from_rgb(150, 157, 206)
        } else {
            Color32::from_rgb(130, 143, 165)
        };
        let paper = rect.shrink2(Vec2::new(rect.width() * 0.15, rect.height() * 0.05));
        painter.rect_filled(paper, 3.0, color);
        if files::image_file(&entry.path) {
            painter.circle_filled(
                paper.min + paper.size() * Vec2::new(0.7, 0.28),
                rect.width() * 0.065,
                Color32::from_rgb(40, 44, 61),
            );
            let points = vec![
                paper.left_bottom() - Vec2::new(-3.0, 4.0),
                paper.center() - Vec2::new(4.0, -1.0),
                paper.center() + Vec2::new(4.0, 7.0),
                paper.right_bottom() - Vec2::new(3.0, 4.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                Color32::from_rgb(70, 77, 109),
                Stroke::NONE,
            ));
        } else {
            for fraction in [0.38, 0.55, 0.72] {
                let y = paper.top() + paper.height() * fraction;
                painter.line_segment(
                    [
                        egui::pos2(paper.left() + 4.0, y),
                        egui::pos2(paper.right() - 4.0, y),
                    ],
                    Stroke::new(1.5, Color32::from_rgb(66, 75, 92)),
                );
            }
        }
    }
    if entry.symlink {
        painter.text(
            rect.right_bottom(),
            egui::Align2::RIGHT_BOTTOM,
            "@",
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([760.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Still Files",
        options,
        Box::new(|context| Ok(Box::new(FileManager::new(context)))),
    )
}
