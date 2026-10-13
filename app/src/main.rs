mod files;
mod images;
mod layout;
mod places;

use eframe::egui::{self, Color32, Key, RichText, Stroke, Vec2};
use files::{Entry, SizeJob, SizeResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const ACCENT: Color32 = Color32::from_rgb(112, 168, 157);
const MUTED: Color32 = Color32::from_rgb(125, 132, 143);

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

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    bookmarks: Vec<PathBuf>,
    removed_places: Vec<PathBuf>,
    view: View,
    zoom: f32,
    hidden: bool,
    dark_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bookmarks: Vec::new(),
            removed_places: Vec::new(),
            view: View::Grid,
            zoom: 1.0,
            hidden: false,
            dark_mode: true,
        }
    }
}

struct Disk {
    total: u64,
    available: u64,
    free: u64,
}

struct NewFolder {
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
    position: usize,
    selected: Option<PathBuf>,
    editing_path: bool,
    focus_path: bool,
    address: String,
    expanded: HashSet<PathBuf>,
    tree_cache: HashMap<PathBuf, Vec<Entry>>,
    size_job: Option<SizeJob>,
    selection_size: Option<SizeResult>,
    disk_job: Option<mpsc::Receiver<Result<Disk, String>>>,
    disk: Option<Disk>,
    home_places: Vec<places::Place>,
    new_folder: Option<NewFolder>,
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
        settings
            .bookmarks
            .retain(|path| !home_places.iter().any(|place| &place.path == path));
        settings.zoom = if settings.zoom.is_finite() {
            settings.zoom.clamp(0.65, 3.6)
        } else {
            1.0
        };
        let requested = std::env::args_os()
            .nth(1)
            .map_or_else(|| home.clone(), PathBuf::from);
        let current = requested.canonicalize().unwrap_or(requested);
        let current = if current.is_dir() { current } else { home };
        let mut app = Self {
            address: current.to_string_lossy().into_owned(),
            history: vec![current.clone()],
            breadcrumb_path: current.clone(),
            thumbnails: images::Thumbnails::new(context.egui_ctx.clone()),
            current,
            config,
            settings,
            entries: Vec::new(),
            position: 0,
            selected: None,
            editing_path: false,
            focus_path: false,
            expanded: HashSet::new(),
            tree_cache: HashMap::new(),
            size_job: None,
            selection_size: None,
            disk_job: None,
            disk: None,
            home_places,
            new_folder: None,
            compression_job: None,
            message: config_error,
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
        match files::entries(&self.current, self.settings.hidden) {
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
            files::entries(&path, self.settings.hidden).map(|entries| (path, entries))
        }) {
            Ok((path, entries)) => {
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
            match files::entries(&path, self.settings.hidden) {
                Ok(entries) => {
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
        self.size_job = None;
        self.selection_size = None;
        if let Some(path) = &path {
            if path.is_dir() {
                self.size_job = Some(files::folder_size(path.clone(), context.clone()));
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
        self.selected = path;
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
        } else if !self.settings.bookmarks.contains(&path) {
            self.settings.bookmarks.push(path);
            self.save();
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
            self.new_folder = Some(NewFolder {
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

    fn shortcuts(&mut self, context: &egui::Context) {
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
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::L)) {
            self.address = self.current.to_string_lossy().into_owned();
            self.editing_path = true;
            self.focus_path = true;
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if self.new_folder.is_some() {
                self.new_folder = None;
            } else if self.editing_path {
                self.editing_path = false;
            } else {
                self.select(None, context);
            }
        }
        if self.new_folder.is_some() || context.wants_keyboard_input() {
            return;
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::D))
            || context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::D))
        {
            self.bookmark();
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowLeft)) {
            self.history_step(false, context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowRight)) {
            self.history_step(true, context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::ArrowUp)) {
            if let Some(parent) = self.current.parent() {
                self.navigate_breadcrumb(parent.to_path_buf(), context);
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::F5))
            || context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::R))
        {
            self.refresh(context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::H)) {
            self.settings.hidden = !self.settings.hidden;
            self.refresh(context);
            self.save();
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter))
            || context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Space))
        {
            if let Some(path) = self.selected.clone() {
                self.open(path, context);
            }
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
                egui::Rect::from_min_max(egui::pos2(row.right() - 256.0, row.top()), row.max);
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
                        .on_hover_text("Toggle editable path / breadcrumbs · Ctrl+L")
                        .clicked()
                    {
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
                        .on_hover_text("Show hidden files · Ctrl+H")
                        .clicked()
                    {
                        self.settings.hidden = !self.settings.hidden;
                        self.refresh(context);
                        self.save();
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

    fn address_bar(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if self.editing_path {
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
        path: &Path,
        label: &str,
        kind: Option<places::Kind>,
        context: &egui::Context,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
        paint_selection(ui, rect, self.current == path, response.hovered());
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
            label,
            egui::pos2(rect.left() + 34.0, rect.center().y),
            rect.width() - 40.0,
            13.0,
        );
        let response = response.on_hover_text(path.to_string_lossy());
        if response.clicked() {
            self.navigate(path.to_path_buf(), context);
        }
        response.context_menu(|ui| {
            if ui.button("Remove bookmark").clicked() {
                if kind.is_some() {
                    if !self
                        .settings
                        .removed_places
                        .iter()
                        .any(|removed| removed == path)
                    {
                        self.settings.removed_places.push(path.to_path_buf());
                    }
                } else {
                    self.settings.bookmarks.retain(|bookmark| bookmark != path);
                }
                self.save();
                ui.close_menu();
            }
        });
    }

    fn sidebar(&mut self, context: &egui::Context) {
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
                        .on_hover_text("Bookmark selected or current folder · D / Ctrl+D")
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
                egui::ScrollArea::vertical()
                    .drag_to_scroll(false)
                    .show(ui, |ui| {
                        for place in self.home_places.clone() {
                            if !self.settings.removed_places.contains(&place.path) {
                                self.bookmark_row(
                                    ui,
                                    &place.path,
                                    place.label,
                                    Some(place.kind),
                                    context,
                                );
                            }
                        }
                        ui.separator();
                        for path in self.settings.bookmarks.clone() {
                            let label = path
                                .file_name()
                                .map_or_else(|| "/".into(), |name| name.to_string_lossy());
                            self.bookmark_row(ui, &path, &label, None, context);
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
                if self.selected.is_some() {
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
                    ui.separator();
                    if let Some(disk) = &self.disk {
                        ui.label(
                            RichText::new(format!("{} free", files::size_label(disk.available)))
                                .size(12.0)
                                .color(ACCENT),
                        );
                    }
                });
            });
            if self.compression_job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Compressing to ZIP…");
                });
            }
            if let Some((completed, total)) = self.thumbnails.progress(context) {
                let label = if completed == total {
                    "Thumbnails ready"
                } else {
                    "Caching thumbnails"
                };
                ui.add(
                    egui::ProgressBar::new(completed as f32 / total as f32)
                        .desired_width(ui.available_width())
                        .desired_height(16.0)
                        .text(format!("{label} · {completed}/{total}")),
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
            ui.add_space(3.0);
        });
    }

    fn entry_response(&mut self, response: egui::Response, entry: &Entry, context: &egui::Context) {
        if entry.directory {
            response.dnd_set_drag_payload(entry.path.clone());
        }
        if response.clicked() || response.secondary_clicked() {
            self.select(Some(entry.path.clone()), context);
        }
        if response.double_clicked() {
            self.open(entry.path.clone(), context);
        }
        let tooltip = if let Some(error) = self.thumbnails.error(&entry.path) {
            format!("{}\nThumbnail unavailable: {error}", entry.path.display())
        } else {
            entry.path.to_string_lossy().into_owned()
        };
        response.on_hover_text(tooltip).context_menu(|ui| {
            if ui.button("Open").clicked() {
                self.open(entry.path.clone(), context);
                ui.close_menu();
            }
            if entry.directory {
                if ui.button("Open Konsole here").clicked() {
                    self.open_terminal(&entry.path);
                    ui.close_menu();
                }
                if ui.button("Add bookmark · Ctrl+D").clicked() {
                    self.add_bookmark(entry.path.clone());
                    ui.close_menu();
                }
            }
            if (entry.directory || files::image_file(&entry.path))
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
        let video = files::video_file(&entry.path);
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
        if video && frame != poster {
            self.thumbnails.get(&entry.path, poster);
        }
        let Some(texture) = self.thumbnails.get(&entry.path, frame) else {
            return false;
        };
        let dimensions = texture.size_vec2();
        let media = self.settings.view == View::Media;
        let scale = (rect.width() / dimensions.x).min(rect.height() / dimensions.y);
        let image_rect = egui::Rect::from_center_size(rect.center(), dimensions * scale);
        let uv_size = image_rect.size() / (dimensions * scale);
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_center_size(egui::pos2(0.5, 0.5), uv_size),
            Color32::WHITE,
        );
        if let Some(progress) = progress {
            let left = egui::pos2(
                rect.left(),
                if media {
                    rect.top() + 2.0
                } else {
                    rect.bottom() + 2.0
                },
            );
            let right = egui::pos2(rect.right(), left.y);
            ui.painter()
                .line_segment([left, right], Stroke::new(2.0, MUTED));
            ui.painter().line_segment(
                [left, egui::pos2(left.x + rect.width() * progress, left.y)],
                Stroke::new(2.0, ACCENT),
            );
        }
        true
    }

    fn grid(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let entries = self.entries.clone();
        let width = 116.0 * self.settings.zoom;
        let height = 126.0 * self.settings.zoom;
        let available = ui.available_width() - ui.spacing().scroll.allocated_width();
        let columns = ((available - 16.0 + 8.0) / (width + 8.0)).floor().max(1.0) as usize;
        let rows = entries.len().div_ceil(columns);
        ui.scope(|ui| {
            egui::ScrollArea::vertical()
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
                                let selected = self.selected.as_ref() == Some(&entry.path);
                                let highlighted = selected || response.hovered();
                                if highlighted {
                                    ui.painter().rect_filled(
                                        rect.shrink(2.0),
                                        8.0,
                                        if selected {
                                            ui.visuals().selection.bg_fill
                                        } else {
                                            ui.visuals().widgets.hovered.weak_bg_fill
                                        },
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
                                {
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
                    self.thumbnails.aspect_ratio(&entry.path)
                }
            })
            .collect();
        let rows = layout::justified_rows(&aspects, width, 192.0 * self.settings.zoom);
        let total_height: f32 = rows.iter().map(|(_, height)| height).sum();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("media")
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
                            self.selected.as_ref() == Some(&entry.path) || response.hovered();
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
                        if entry.directory || highlighted {
                            let label = egui::Rect::from_min_max(
                                egui::pos2(rect.left(), (rect.bottom() - 26.0).max(rect.top())),
                                rect.max,
                            );
                            ui.painter()
                                .rect_filled(label, 0.0, ui.visuals().panel_fill);
                            paint_name(
                                ui,
                                &entry.name,
                                egui::pos2(label.left() + 6.0, label.center().y),
                                label.width() - 12.0,
                                12.0,
                            );
                        }
                        if highlighted {
                            ui.painter().rect_stroke(
                                rect.shrink(1.0),
                                0.0,
                                Stroke::new(2.0, ACCENT),
                                egui::StrokeKind::Inside,
                            );
                        }
                        self.entry_response(response, entry, context);
                    }
                    y += height;
                }
            });
    }

    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        entries: Vec<Entry>,
        depth: usize,
        context: &egui::Context,
    ) {
        for entry in entries {
            ui.push_id(&entry.path, |ui| {
                let height = (30.0 * self.settings.zoom).max(32.0);
                let (row, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), height),
                    egui::Sense::hover(),
                );
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
                            match files::entries(&entry.path, self.settings.hidden) {
                                Ok(children) => {
                                    self.tree_cache.insert(entry.path.clone(), children);
                                    self.expanded.insert(entry.path.clone());
                                }
                                Err(error) => {
                                    self.message = Some(format!("Cannot expand directory: {error}"))
                                }
                            }
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
                    self.selected.as_ref() == Some(&entry.path),
                    response.hovered(),
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
                let size_width = if entry.directory { 0.0 } else { 90.0 };
                paint_name(
                    ui,
                    &entry.name,
                    egui::pos2(name_left, row.center().y),
                    (row.right() - name_left - size_width - 8.0).max(1.0),
                    14.0,
                );
                if !entry.directory {
                    ui.painter().text(
                        egui::pos2(row.right() - 8.0, row.center().y),
                        egui::Align2::RIGHT_CENTER,
                        files::size_label(entry.size),
                        egui::FontId::proportional(11.0),
                        MUTED,
                    );
                }
                self.entry_response(response, &entry, context);
                if self.expanded.contains(&entry.path) && depth < 64 {
                    if let Some(children) = self.tree_cache.get(&entry.path).cloned() {
                        self.tree(ui, children, depth + 1, context);
                    }
                }
            });
        }
    }

    fn new_folder_window(&mut self, context: &egui::Context) {
        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        if let Some(folder) = &mut self.new_folder {
            egui::Window::new("New folder")
                .id(egui::Id::new("new-folder"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(context, |ui| {
                    ui.label(folder.parent.to_string_lossy());
                    let response = ui
                        .add(egui::TextEdit::singleline(&mut folder.name).hint_text("Folder name"));
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
        if !open || cancel {
            self.new_folder = None;
        } else if create {
            if let Some(folder) = &mut self.new_folder {
                match files::create_directory(&folder.parent, &folder.name) {
                    Ok(path) => {
                        self.new_folder = None;
                        self.refresh(context);
                        self.select(Some(path), context);
                    }
                    Err(error) => folder.error = Some(format!("Cannot create folder: {error}")),
                }
            }
        }
    }
}

impl eframe::App for FileManager {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        self.poll(context);
        self.shortcuts(context);
        self.toolbar(context);
        self.statusbar(context);
        self.sidebar(context);
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&context.style()).inner_margin(0))
            .show(context, |ui| {
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
                if self.entries.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("This folder is empty").color(MUTED));
                    });
                } else {
                    match self.settings.view {
                        View::Grid => self.grid(ui, context),
                        View::Media => self.media(ui, context),
                        View::Tree => {
                            egui::ScrollArea::both()
                                .auto_shrink([false, false])
                                .id_salt("tree")
                                .drag_to_scroll(false)
                                .show(ui, |ui| {
                                    self.tree(ui, self.entries.clone(), 0, context);
                                });
                        }
                    }
                }
                if background.clicked() || background.secondary_clicked() {
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
