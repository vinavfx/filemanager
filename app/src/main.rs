mod bookmarks;
mod conversion;
mod drag;
mod files;
mod icons;
mod images;
mod instance;
mod layout;
mod mounts;
mod operations;
mod places;
mod properties;
mod search;
#[path = "../../shared/thumbnail_cache.rs"]
mod thumbnail_cache;
mod watch;

use eframe::egui::{self, Color32, Key, RichText, Stroke, Vec2};
use files::{Entry, SizeJob, SizeResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

const ACCENT: Color32 = Color32::from_rgb(112, 168, 157);
const MUTED: Color32 = Color32::from_rgb(125, 132, 143);
const TREE_SIZE_WIDTH: f32 = 110.0;
const TREE_DATE_WIDTH: f32 = 156.0;
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

fn with_scrollbar_color<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui, Arc<egui::Style>) -> R,
) -> R {
    let content_style = Arc::clone(ui.style());
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.bg_fill = ACCENT;
    widgets.hovered.bg_fill = ACCENT;
    widgets.active.bg_fill = ACCENT;
    let result = add_contents(ui, Arc::clone(&content_style));
    ui.set_style(content_style);
    result
}

#[derive(Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum View {
    #[default]
    Tree,
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
    last_directory: Option<PathBuf>,
    removed_places: Vec<PathBuf>,
    entry_order: HashMap<PathBuf, Vec<PathBuf>>,
    media_players: HashMap<String, String>,
    view: View,
    media_return_view: View,
    tree_sort: TreeSort,
    tree_descending: bool,
    tree_zoom: f32,
    #[serde(alias = "zoom")]
    grid_zoom: f32,
    media_zoom: f32,
    hidden: bool,
    sequences: bool,
    #[serde(skip)]
    recursive_tree: bool,
    #[serde(skip)]
    recursive_grid: bool,
    #[serde(skip)]
    recursive_media: bool,
    dark_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_directory: None,
            removed_places: Vec::new(),
            entry_order: HashMap::new(),
            media_players: HashMap::new(),
            view: View::Tree,
            media_return_view: View::Tree,
            tree_sort: TreeSort::Name,
            tree_descending: false,
            tree_zoom: 1.0,
            grid_zoom: 1.0,
            media_zoom: 1.0,
            hidden: false,
            sequences: true,
            recursive_tree: false,
            recursive_grid: false,
            recursive_media: false,
            dark_mode: true,
        }
    }
}

impl Settings {
    fn zoom(&self) -> f32 {
        match self.view {
            View::Tree => self.tree_zoom,
            View::Grid => self.grid_zoom,
            View::Media => self.media_zoom,
        }
    }

    fn zoom_mut(&mut self) -> &mut f32 {
        match self.view {
            View::Tree => &mut self.tree_zoom,
            View::Grid => &mut self.grid_zoom,
            View::Media => &mut self.media_zoom,
        }
    }

    fn recursive(&self) -> bool {
        match self.view {
            View::Tree => self.recursive_tree,
            View::Grid => self.recursive_grid,
            View::Media => self.recursive_media,
        }
    }

    fn recursive_mut(&mut self) -> &mut bool {
        match self.view {
            View::Tree => &mut self.recursive_tree,
            View::Grid => &mut self.recursive_grid,
            View::Media => &mut self.recursive_media,
        }
    }

    fn set_view(&mut self, view: View) {
        if view == View::Media && self.view != View::Media {
            self.media_return_view = self.view;
            if self.recursive() {
                self.recursive_media = true;
            }
        }
        self.view = view;
    }
}

struct Disk {
    total: u64,
    available: u64,
    free: u64,
}

struct NameDialog {
    preview: Option<Arc<Vec<Entry>>>,
    previous_selection: Option<DirectorySelection>,
    anchor: Option<PathBuf>,
    source: Option<PathBuf>,
    sources: Vec<PathBuf>,
    submit: bool,
    parent: PathBuf,
    name: String,
    focus: bool,
    error: Option<String>,
}

impl NameDialog {
    fn creation_order(&self, path: &Path) -> Option<Vec<PathBuf>> {
        let (_, entries) = self.preview.as_ref()?.split_last()?;
        let mut order: Vec<_> = entries.iter().map(|entry| entry.path.clone()).collect();
        let index = self
            .anchor
            .as_ref()
            .and_then(|anchor| order.iter().position(|entry| entry == anchor))
            .map_or(0, |index| index + 1);
        order.insert(index, path.to_path_buf());
        Some(order)
    }

    fn editing_path(&self) -> Option<&PathBuf> {
        self.source.as_ref().or_else(|| {
            self.preview
                .as_ref()
                .and_then(|entries| entries.last())
                .map(|entry| &entry.path)
        })
    }
}

#[derive(Default)]
struct PathCompletion {
    candidates: Vec<String>,
    index: Option<usize>,
}

impl PathCompletion {
    fn new(address: &str, current: &Path, hidden: bool) -> Self {
        if address == "~" {
            return Self {
                candidates: vec!["~/".to_owned()],
                index: None,
            };
        }
        let (base, prefix) = address
            .rfind('/')
            .map_or(("", address), |index| address.split_at(index + 1));
        let directory = if let Some(tail) = base.strip_prefix("~/") {
            let Some(home) = std::env::var_os("HOME") else {
                return Self::default();
            };
            PathBuf::from(home).join(tail)
        } else {
            current.join(base)
        };
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Self::default();
        };
        let mut candidates: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                if !name.starts_with(prefix)
                    || (!hidden && !prefix.starts_with('.') && name.starts_with('.'))
                    || !entry.path().is_dir()
                {
                    return None;
                }
                Some(format!("{base}{name}/"))
            })
            .collect();
        candidates.sort();
        Self {
            candidates,
            index: None,
        }
    }

    fn cycle(&mut self, backward: bool) -> Option<&str> {
        let count = self.candidates.len();
        if count == 0 {
            return None;
        }
        let index = match self.index {
            None if backward => count - 1,
            None => 0,
            Some(index) if backward => (index + count - 1) % count,
            Some(index) => (index + 1) % count,
        };
        self.index = Some(index);
        Some(&self.candidates[index])
    }
}

struct TreeLayout {
    sort: TreeSort,
    descending: bool,
    zoom: f32,
    expanded: HashSet<PathBuf>,
    items: Arc<Vec<(Entry, egui::Pos2)>>,
}

#[derive(Default)]
struct DirectorySelection {
    selected: Option<PathBuf>,
    last_selected: Option<PathBuf>,
    paths: HashSet<PathBuf>,
    anchor: Option<PathBuf>,
    visual_line: bool,
}

struct DirectoryFrame {
    graphics: egui::layers::GraphicLayers,
    _textures: Vec<egui::TextureHandle>,
}

enum FileOperation {
    Rename(Vec<(PathBuf, PathBuf)>),
    Move(Vec<(PathBuf, PathBuf)>),
    Copy(Vec<operations::YankedFile>, PathBuf),
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    EmptyTrash,
    Extract(PathBuf),
    Undo,
    Redo,
}

enum OperationUpdate {
    Visibility(Vec<PathBuf>, bool),
    Progress(usize, usize, PathBuf),
    Finished(operations::History, operations::Outcome),
}

struct OperationJob {
    receiver: mpsc::Receiver<OperationUpdate>,
    previous_selection: Option<(PathBuf, DirectorySelection)>,
    trash_paths: HashSet<PathBuf>,
    pending_trash: Vec<PathBuf>,
    completed_offset: usize,
    outcome: operations::Outcome,
    preserve_selection: bool,
    visual_changed: bool,
    restored_paths: Vec<PathBuf>,
    hidden_paths: Vec<PathBuf>,
    pending_visibility: Vec<mpsc::Receiver<Vec<PathBuf>>>,
    label: &'static str,
    completed: usize,
    total: usize,
    path: PathBuf,
}

struct PastePlacement {
    directory: PathBuf,
    anchor: Option<PathBuf>,
    order: Vec<PathBuf>,
}

struct DirectoryContents {
    entries: Vec<Entry>,
    expanded: HashSet<PathBuf>,
    tree_cache: HashMap<PathBuf, Vec<Entry>>,
}

struct CachedViewContents {
    entries: Arc<Vec<Entry>>,
    tree_cache: HashMap<PathBuf, Vec<Entry>>,
    media_layout: MediaLayout,
}

struct CachedMediaDirectory {
    path: PathBuf,
    hidden: bool,
    sequences: bool,
    entries: Arc<Vec<Entry>>,
    directories: Arc<Vec<files::MediaDirectory>>,
    layout: MediaLayout,
    scan: Option<files::MediaScan>,
}

#[derive(Default)]
struct MediaLayout {
    source: std::sync::Weak<Vec<Entry>>,
    indices: HashMap<PathBuf, usize>,
    items: Arc<Vec<(Entry, egui::Pos2, f32)>>,
    complete: bool,
    thumbnail_revision: u64,
    updated: Option<std::time::Instant>,
    aspects: HashMap<PathBuf, f32>,
    paths: Vec<PathBuf>,
    rows: Vec<(std::ops::Range<usize>, f32)>,
    width: f32,
    target: f32,
}

struct MarqueeSelection {
    origin: egui::Pos2,
    initial: HashSet<PathBuf>,
}

struct EntryMenu {
    root: egui::menu::MenuRootManager,
    entry: Entry,
    use_selection: bool,
    cursor: usize,
    items: usize,
    activate: bool,
    count: usize,
    pending_g: bool,
}

struct FileManager {
    external_drag: drag::ExternalDrag,
    directory_refresh: Option<mpsc::Receiver<Result<DirectoryContents, String>>>,
    cached_view: Option<CachedViewContents>,
    directory_frame: Option<DirectoryFrame>,
    preparing_directory: bool,
    wait_for_thumbnails: bool,
    directory_ready: bool,
    prepared_view: Vec<(PathBuf, egui::Rect)>,
    settings: Settings,
    config: PathBuf,
    current: PathBuf,
    breadcrumb_path: PathBuf,
    thumbnails: images::Thumbnails,
    icons: icons::Icons,
    mounts: mounts::Mounts,
    entries: Arc<Vec<Entry>>,
    folder_counts: files::FolderCounts,
    media_scan: Option<files::MediaScan>,
    media_cache: Vec<CachedMediaDirectory>,
    media_directories: Arc<Vec<files::MediaDirectory>>,
    directory_watch: Option<watch::DirectoryWatch>,
    history: Vec<PathBuf>,
    operations: operations::History,
    operation_job: Option<OperationJob>,
    removed_entries: HashMap<PathBuf, Entry>,
    position: usize,
    selected: Option<PathBuf>,
    last_selected: Option<PathBuf>,
    selection: HashSet<PathBuf>,
    entry_menu: Option<EntryMenu>,
    marquee_selection: Option<MarqueeSelection>,
    copied_files: Vec<operations::YankedFile>,
    paste_placement: Option<PastePlacement>,
    selection_anchor: Option<PathBuf>,
    directory_selections: HashMap<PathBuf, DirectorySelection>,
    visual_line: bool,
    motion_count: usize,
    visible_paths: Vec<PathBuf>,
    scroll_to_selection: bool,
    media_layout: MediaLayout,
    media_scroll_target: Option<f32>,
    media_scrolling_until: f64,
    center_selection: bool,
    page_scroll_delta: f32,
    pending_g: bool,
    pending_z: bool,
    pending_y: bool,
    pending_d: bool,
    sidebar_focused: bool,
    pending_window_command: bool,
    sidebar_cursor: Option<usize>,
    sidebar_cursor_y: Option<f32>,
    pane_selection_y: Option<f32>,
    scroll_to_bookmark: bool,
    editing_path: bool,
    focus_path: bool,
    search: Option<search::Search>,
    find_query: Option<String>,
    focus_find: bool,
    address: String,
    path_completion: PathCompletion,
    expanded: HashSet<PathBuf>,
    directory_expansions: HashMap<PathBuf, HashSet<PathBuf>>,
    tree_cache: HashMap<PathBuf, Vec<Entry>>,
    tree_layout: Option<TreeLayout>,
    size_job: Option<SizeJob>,
    selection_size: Option<SizeResult>,
    disk_job: Option<mpsc::Receiver<Result<Disk, String>>>,
    disk: Option<Disk>,
    home_places: Vec<places::Place>,
    bookmarks: bookmarks::Bookmarks,
    bookmark_undo: Vec<(bookmarks::Bookmark, Option<places::Kind>, usize)>,
    bookmark_redo: Vec<(bookmarks::Bookmark, Option<places::Kind>, usize)>,
    new_folder: Option<NameDialog>,
    confirm_empty_trash: bool,
    pending_delete: Vec<PathBuf>,
    trash_focus: Option<bool>,
    compression_job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    conversion_job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    message: Option<String>,
    message_deadline: std::time::Instant,
}

impl FileManager {
    fn new(
        context: &egui::Context,
        external_drag: drag::ExternalDrag,
        requested: Option<PathBuf>,
    ) -> Self {
        let mut style = (*context.style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(10.0, 7.0);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.menu_margin = egui::Margin::same(10);
        context.set_style(style);
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
        settings.view = View::Tree;
        apply_theme(context, settings.dark_mode);
        let home_places = places::defaults(&home);
        let mut bookmarks = bookmarks::Bookmarks::new(&home);
        let bookmark_error = bookmarks
            .reload()
            .err()
            .map(|error| format!("Cannot read shared bookmarks: {error}"));
        for zoom in [
            &mut settings.tree_zoom,
            &mut settings.grid_zoom,
            &mut settings.media_zoom,
        ] {
            *zoom = if zoom.is_finite() {
                zoom.clamp(0.65, 3.6)
            } else {
                1.0
            };
        }
        let requested = requested
            .or_else(|| settings.last_directory.clone())
            .unwrap_or_else(|| home.clone());
        let current = requested.canonicalize().unwrap_or(requested);
        let current = if current.is_dir() || files::is_trash(&current) {
            current
        } else {
            home
        };
        let mut app = Self {
            external_drag,
            directory_refresh: None,
            cached_view: None,
            directory_frame: None,
            preparing_directory: false,
            wait_for_thumbnails: false,
            directory_ready: true,
            prepared_view: Vec::new(),
            address: current.to_string_lossy().into_owned(),
            history: vec![current.clone()],
            operations: operations::History::default(),
            operation_job: None,
            removed_entries: HashMap::new(),
            breadcrumb_path: current.clone(),
            thumbnails: images::Thumbnails::new(context.clone()),
            icons: icons::Icons::new(context.clone()),
            mounts: mounts::Mounts::new(),
            current,
            config,
            settings,
            entries: Arc::default(),
            folder_counts: files::FolderCounts::new(context.clone()),
            media_scan: None,
            media_cache: Vec::new(),
            media_directories: Arc::default(),
            directory_watch: match watch::DirectoryWatch::new(context.clone()) {
                Ok(watch) => Some(watch),
                Err(error) => {
                    eprintln!("Cannot monitor directory changes: {error}");
                    None
                }
            },
            position: 0,
            selected: None,
            last_selected: None,
            selection: HashSet::new(),
            entry_menu: None,
            marquee_selection: None,
            copied_files: Vec::new(),
            paste_placement: None,
            selection_anchor: None,
            directory_selections: HashMap::new(),
            visual_line: false,
            motion_count: 0,
            visible_paths: Vec::new(),
            scroll_to_selection: false,
            media_layout: MediaLayout::default(),
            media_scroll_target: None,
            media_scrolling_until: 0.0,
            center_selection: false,
            page_scroll_delta: 0.0,
            pending_g: false,
            pending_z: false,
            pending_y: false,
            pending_d: false,
            sidebar_focused: false,
            pending_window_command: false,
            sidebar_cursor: None,
            sidebar_cursor_y: None,
            pane_selection_y: None,
            scroll_to_bookmark: false,
            editing_path: false,
            focus_path: false,
            search: None,
            find_query: None,
            focus_find: false,
            path_completion: PathCompletion::default(),
            expanded: HashSet::new(),
            directory_expansions: HashMap::new(),
            tree_cache: HashMap::new(),
            tree_layout: None,
            size_job: None,
            selection_size: None,
            disk_job: None,
            disk: None,
            home_places,
            bookmarks,
            bookmark_undo: Vec::new(),
            bookmark_redo: Vec::new(),
            new_folder: None,
            confirm_empty_trash: false,
            pending_delete: Vec::new(),
            trash_focus: Some(false),
            compression_job: None,
            conversion_job: None,
            message: config_error.or(bookmark_error),
            message_deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        };
        app.refresh(context);
        app.save();
        app
    }

    fn set_message(&mut self, message: Option<String>) {
        self.message = message;
        self.message_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    }

    fn save(&mut self) {
        self.settings.last_directory = Some(self.current.clone());
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
            self.set_message(Some(format!("Cannot save settings: {error}")));
        }
    }

    fn start_media_scan(&mut self, context: &egui::Context) {
        self.media_directories = Arc::default();
        self.media_layout.complete = false;
        self.directory_refresh = None;
        self.media_scan = None;
        Arc::make_mut(&mut self.entries).retain(|entry| {
            !entry.directory
                && (entry.sequence.is_some()
                    || files::image_file(&entry.path)
                    || files::video_file(&entry.path))
        });
        self.tree_cache.clear();
        self.tree_layout = None;
        self.folder_counts.cancel();
        self.thumbnails.cancel();
        self.media_scan = Some(files::MediaScan::new(
            self.current.clone(),
            self.settings.hidden,
            self.settings.sequences,
            context.clone(),
        ));
        self.retain_visible_selection(context);
    }

    fn set_view(&mut self, view: View, context: &egui::Context) {
        let recursive = self.settings.recursive();
        self.settings.set_view(view);
        if self.settings.recursive() != recursive {
            let cached = self.cached_view.take();
            let previous = self
                .directory_refresh
                .is_none()
                .then(|| CachedViewContents {
                    entries: Arc::clone(&self.entries),
                    tree_cache: std::mem::take(&mut self.tree_cache),
                    media_layout: std::mem::take(&mut self.media_layout),
                });
            let contents = cached.unwrap_or_else(|| CachedViewContents {
                entries: Arc::default(),
                tree_cache: HashMap::new(),
                media_layout: MediaLayout::default(),
            });
            self.entries = contents.entries;
            self.tree_cache = contents.tree_cache;
            self.media_layout = contents.media_layout;
            self.tree_layout = None;
            self.directory_frame = None;
            self.preparing_directory = false;
            self.refresh(context);
            self.cached_view = previous;
        }
        self.save();
    }

    fn refresh(&mut self, context: &egui::Context) {
        self.cached_view = None;
        self.media_cache.clear();
        self.media_directories = Arc::default();
        self.wait_for_thumbnails = false;
        self.media_scan = None;
        self.directory_refresh = None;
        if self.settings.recursive() && !files::is_trash(&self.current) {
            self.start_media_scan(context);
        } else {
            let current = self.current.clone();
            let expanded = self.expanded.clone();
            let hidden = self.settings.hidden;
            let sequences = self.settings.sequences;
            let context = context.clone();
            let (sender, receiver) = mpsc::channel();
            self.directory_refresh = Some(receiver);
            std::thread::spawn(move || {
                let result = files::entries(&current, hidden, sequences)
                    .map(|entries| {
                        let tree_cache = expanded
                            .iter()
                            .filter_map(|path| {
                                files::entries(path, hidden, sequences)
                                    .ok()
                                    .map(|entries| (path.clone(), entries))
                            })
                            .collect();
                        DirectoryContents {
                            entries,
                            expanded,
                            tree_cache,
                        }
                    })
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
                context.request_repaint();
            });
        }
        self.refresh_disk(context);
        if let Some(search) = self.search.take() {
            let mut replacement =
                search::Search::new(self.current.clone(), self.settings.hidden, context.clone());
            replacement.query.clone_from(&search.query);
            replacement.submit();
            self.search = Some(replacement);
        }
    }

    fn retain_visible_selection(&mut self, context: &egui::Context) {
        if !self.selection.is_empty() {
            let visible: HashSet<_> = if self.settings.view == View::Tree {
                self.tree_layout()
                    .iter()
                    .map(|(entry, _)| entry.path.clone())
                    .collect()
            } else {
                self.entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect()
            };
            let previous_count = self.selection.len();
            self.selection.retain(|path| visible.contains(path));
            if self.selection.len() != previous_count {
                let selection = std::mem::take(&mut self.selection);
                let selected = self
                    .selected
                    .clone()
                    .filter(|path| selection.contains(path))
                    .or_else(|| selection.iter().min().cloned());
                let anchor = self
                    .selection_anchor
                    .clone()
                    .filter(|path| selection.contains(path))
                    .or_else(|| selected.clone());
                let visual_line = self.visual_line && selected.is_some();
                self.select(selected, context);
                self.selection = selection;
                self.selection_anchor = anchor;
                self.visual_line = visual_line;
                if self.selection.len() > 1 {
                    self.size_job = None;
                    self.selection_size = None;
                }
            }
        }
    }

    fn refresh_disk(&mut self, context: &egui::Context) {
        if files::is_trash(&self.current) {
            self.disk = None;
            self.disk_job = None;
            return;
        }
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

    fn cache_media_directory(&mut self, scan: Option<files::MediaScan>, layout: MediaLayout) {
        if self.operation_job.is_some() || self.entries.len() > 50_000 {
            return;
        }
        self.media_cache
            .retain(|cached| cached.path != self.current);
        while self.media_cache.len() >= 8
            || self
                .media_cache
                .iter()
                .map(|cached| cached.entries.len())
                .sum::<usize>()
                + self.entries.len()
                > 50_000
        {
            self.media_cache.remove(0);
        }
        self.media_cache.push(CachedMediaDirectory {
            path: self.current.clone(),
            hidden: self.settings.hidden,
            sequences: self.settings.sequences,
            entries: Arc::clone(&self.entries),
            directories: Arc::clone(&self.media_directories),
            layout,
            scan,
        });
    }

    fn load_directory(&mut self, path: PathBuf, mut entries: Vec<Entry>, context: &egui::Context) {
        self.cached_view = None;
        if let Some(order) = self.settings.entry_order.get(&path) {
            order_entries(&mut entries, order);
        }
        if let Some(scan) = self.media_scan.take() {
            scan.pause();
            let layout = std::mem::take(&mut self.media_layout);
            self.cache_media_directory(Some(scan), layout);
        } else if let Some(cached) = self.media_cache.iter_mut().find(|cached| {
            cached.path == self.current && Arc::ptr_eq(&cached.entries, &self.entries)
        }) {
            cached.layout = std::mem::take(&mut self.media_layout);
        }
        self.media_layout = MediaLayout::default();
        self.media_scroll_target = None;
        self.media_scrolling_until = 0.0;
        self.media_directories = Arc::default();
        self.wait_for_thumbnails = false;
        self.directory_refresh = None;
        self.directory_selections.insert(
            self.current.clone(),
            DirectorySelection {
                selected: self.selected.take(),
                last_selected: self.last_selected.take(),
                paths: std::mem::take(&mut self.selection),
                anchor: self.selection_anchor.take(),
                visual_line: self.visual_line,
            },
        );
        self.preparing_directory = self.directory_frame.is_some();
        self.find_query = None;
        self.directory_expansions
            .insert(self.current.clone(), std::mem::take(&mut self.expanded));
        self.expanded = self.directory_expansions.remove(&path).unwrap_or_default();
        self.current = path;
        self.save();
        self.folder_counts.restore(&mut entries);
        self.entries = Arc::new(entries);
        self.folder_counts.cancel();
        self.folder_counts.request(&self.entries);
        self.address = self.current.to_string_lossy().into_owned();
        self.tree_cache.clear();
        self.tree_layout = None;
        if files::is_trash(&self.current) {
            self.media_scan = None;
            self.select(None, context);
            self.refresh(context);
        } else if self.settings.recursive() {
            self.select(None, context);
            if let Some(index) = self.media_cache.iter().position(|cached| {
                cached.path == self.current
                    && cached.hidden == self.settings.hidden
                    && cached.sequences == self.settings.sequences
            }) {
                let mut cached = self.media_cache.remove(index);
                if files::media_directories_changed(&cached.directories) {
                    self.start_media_scan(context);
                } else {
                    self.media_scan = cached.scan.take();
                    if let Some(scan) = &self.media_scan {
                        scan.resume();
                    }
                    self.entries = Arc::clone(&cached.entries);
                    self.media_directories = Arc::clone(&cached.directories);
                    self.media_layout = std::mem::take(&mut cached.layout);
                    if self.media_scan.is_none() {
                        self.media_cache.push(cached);
                    }
                    self.restore_directory_selection(context);
                }
            } else {
                self.start_media_scan(context);
            }
        } else {
            self.media_scan = None;
            for expanded in std::mem::take(&mut self.expanded) {
                if expanded.is_dir() {
                    self.expand_directory(&expanded);
                }
            }
            self.restore_directory_selection(context);
        }
    }

    fn restore_directory_selection(&mut self, context: &egui::Context) {
        let mut state = self
            .directory_selections
            .remove(&self.current)
            .unwrap_or_default();
        let visible: HashSet<_> = if self.settings.view == View::Tree {
            self.tree_layout()
                .iter()
                .map(|(entry, _)| entry.path.clone())
                .collect()
        } else {
            self.entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect()
        };
        state.paths.retain(|path| visible.contains(path));
        let selected = state
            .selected
            .filter(|path| state.paths.contains(path))
            .or_else(|| state.paths.iter().min().cloned());
        self.select(selected, context);
        self.selection_anchor = state
            .anchor
            .filter(|path| state.paths.contains(path))
            .or_else(|| self.selected.clone());
        self.last_selected = state.last_selected.filter(|path| visible.contains(path));
        self.visual_line = state.visual_line && self.selected.is_some();
        self.selection = state.paths;
        if self.selection.len() > 1 {
            self.size_job = None;
            self.selection_size = None;
        }
        self.scroll_to_selection = self.selected.is_some();
    }

    fn navigate(&mut self, path: PathBuf, context: &egui::Context) {
        if files::is_trash(&path) {
            self.search = None;
            self.thumbnails.cancel();
            self.new_folder = None;
            if path != self.current {
                self.history.truncate(self.position + 1);
                self.history.push(path.clone());
                self.position = self.history.len() - 1;
            }
            self.breadcrumb_path = path.clone();
            self.load_directory(path, Vec::new(), context);
            return;
        }
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.set_message(Some(format!("Cannot open directory: {error}")));
                return;
            }
        };
        if path == self.current {
            self.address = path.to_string_lossy().into_owned();
            if self.search.take().is_some() {
                self.select(None, context);
            }
            return;
        }
        let entries = if self.settings.recursive() {
            std::fs::read_dir(&path).map(|_| Vec::new())
        } else {
            files::entries(&path, self.settings.hidden, self.settings.sequences)
        };
        match entries {
            Ok(entries) => {
                self.search = None;
                self.thumbnails.cancel();
                self.new_folder = None;
                if path != self.current {
                    self.history.truncate(self.position + 1);
                    self.history.push(path.clone());
                    self.position = self.history.len() - 1;
                }
                self.breadcrumb_path = path.clone();
                self.load_directory(path, entries, context);
                self.refresh_disk(context);
            }
            Err(error) => self.set_message(Some(format!("Cannot open directory: {error}"))),
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
            if files::is_trash(&path) {
                self.search = None;
                self.thumbnails.cancel();
                self.new_folder = None;
                self.position = next;
                self.breadcrumb_path = path.clone();
                self.load_directory(path, Vec::new(), context);
                return;
            }
            let entries = if self.settings.recursive() {
                std::fs::read_dir(&path).map(|_| Vec::new())
            } else {
                files::entries(&path, self.settings.hidden, self.settings.sequences)
            };
            match entries {
                Ok(entries) => {
                    self.search = None;
                    self.thumbnails.cancel();
                    self.new_folder = None;
                    self.position = next;
                    if !self.breadcrumb_path.starts_with(&path) {
                        self.breadcrumb_path = path.clone();
                    }
                    self.load_directory(path, entries, context);
                    self.refresh_disk(context);
                }
                Err(error) => self.set_message(Some(format!("Cannot open directory: {error}"))),
            }
        }
    }

    fn select(&mut self, path: Option<PathBuf>, context: &egui::Context) {
        self.visual_line = false;
        self.motion_count = 0;
        self.pending_g = false;
        self.pending_z = false;
        self.pending_y = false;
        self.pending_d = false;
        self.center_selection = false;
        self.size_job = None;
        self.selection_size = None;
        if let Some(path) = &path {
            self.last_selected = Some(path.clone());
            let entry = self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .find(|entry| &entry.path == path);
            if let Some(entry) = entry.filter(|entry| !entry.directory) {
                self.selection_size = Some(SizeResult {
                    bytes: entry.size,
                    incomplete: false,
                });
            } else {
                self.size_job = Some(files::folder_size(path.clone(), context.clone()));
            }
        }
        self.selection = path.iter().cloned().collect();
        self.selection_anchor = path.clone();
        self.selected = path;
    }

    fn exit_visual(&mut self, context: &egui::Context) {
        let first = self
            .visible_paths
            .iter()
            .find(|path| self.selection.contains(*path))
            .cloned()
            .or_else(|| self.selected.clone());
        self.select(first, context);
    }

    fn yank(&mut self) {
        let selected: Vec<_> = self
            .visible_paths
            .iter()
            .filter(|path| self.selection.contains(*path))
            .filter(|path| {
                !path
                    .ancestors()
                    .skip(1)
                    .any(|parent| self.selection.contains(parent))
            })
            .cloned()
            .collect();
        if selected.is_empty() {
            return;
        }
        self.copied_files = selected
            .iter()
            .flat_map(|path| {
                self.entries
                    .iter()
                    .chain(self.tree_cache.values().flatten())
                    .find(|entry| &entry.path == path)
                    .and_then(|entry| entry.sequence.as_ref())
                    .map_or_else(|| vec![path.clone()], |frames| frames.as_ref().clone())
            })
            .map(operations::YankedFile::from)
            .collect();
        self.visual_line = false;
        self.set_message(Some(format!("Yanked {} items.", selected.len())));
    }

    fn receive_external_files(&mut self, context: &egui::Context) {
        let dropped = context.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        if dropped.is_empty() {
            return;
        }
        if self.operation_job.is_some() || self.new_folder.is_some() {
            self.set_message(Some(
                "Finish the current operation before dropping files.".into(),
            ));
            return;
        }
        if files::is_trash(&self.current) {
            self.set_message(Some("Open a folder before dropping files.".into()));
            return;
        }
        let paths: Vec<_> = dropped.into_iter().filter_map(|file| file.path).collect();
        if paths.is_empty() {
            self.set_message(Some(
                "Only local files and folders can be dropped here.".into(),
            ));
            return;
        }
        let items = paths
            .iter()
            .filter(|path| {
                !path
                    .ancestors()
                    .skip(1)
                    .any(|parent| paths.iter().any(|other| other == parent))
            })
            .cloned()
            .map(operations::YankedFile::from)
            .collect();
        self.start_operation(FileOperation::Copy(items, self.current.clone()), context);
    }

    fn paste(&mut self, context: &egui::Context) {
        if self.operation_job.is_some()
            || self.new_folder.is_some()
            || files::is_trash(&self.current)
        {
            return;
        }
        if self.copied_files.is_empty() {
            self.set_message(Some(
                "Nothing to paste. Select files and press yy, or y in Visual mode.".into(),
            ));
            return;
        }
        let into_folder = self.settings.view == View::Tree
            && self
                .selected
                .as_ref()
                .is_some_and(|path| path.is_dir() && self.expanded.contains(path));
        let directory = if into_folder {
            self.selected
                .clone()
                .unwrap_or_else(|| self.current.clone())
        } else {
            self.selected
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(&self.current)
                .to_path_buf()
        };
        let siblings = if self.settings.view == View::Tree {
            let mut entries = if directory == self.current {
                self.entries.as_ref().clone()
            } else {
                self.tree_cache.get(&directory).cloned().unwrap_or_default()
            };
            self.sort_tree_entries(&mut entries);
            entries.into_iter().map(|entry| entry.path).collect()
        } else {
            self.entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>()
        };
        let anchor = if into_folder {
            None
        } else {
            siblings
                .iter()
                .rev()
                .find(|path| self.selection.contains(*path))
                .cloned()
                .or_else(|| self.selected.clone())
        };
        self.paste_placement = Some(PastePlacement {
            directory: if self.settings.view == View::Tree {
                directory.clone()
            } else {
                self.current.clone()
            },
            anchor,
            order: siblings,
        });
        self.visual_line = false;
        self.start_operation(
            FileOperation::Copy(self.copied_files.clone(), directory),
            context,
        );
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
            self.set_message(Some(format!("Cannot save shared bookmarks: {error}")));
        }
    }

    fn bookmark(&mut self) {
        match self.selected.clone() {
            Some(path) if path.is_dir() => self.add_bookmark(path),
            None => self.add_bookmark(self.current.clone()),
            Some(_) => self.set_message(Some(
                "Select a folder or clear the selection to bookmark the current directory.".into(),
            )),
        }
    }

    fn open(&mut self, path: PathBuf, context: &egui::Context) {
        self.visual_line = false;
        if path.is_dir() {
            self.navigate(path, context);
            return;
        }
        let multimedia = files::image_file(&path)
            || files::video_file(&path)
            || self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .any(|entry| entry.path == path && entry.sequence.is_some());
        let application = if multimedia {
            path.extension()
                .and_then(|extension| extension.to_str())
                .and_then(|extension| self.settings.media_players.get(&extension.to_lowercase()))
                .map(String::as_str)
                .filter(|application| matches!(*application, "player" | "vlc" | "mpv"))
                .unwrap_or("player")
        } else {
            "xdg-open"
        }
        .to_owned();
        self.open_media(&path, &application);
    }

    fn choose_media_player(&mut self, path: &Path, application: &str) {
        if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
            self.settings
                .media_players
                .insert(extension.to_lowercase(), application.to_owned());
            self.save();
        }
        self.open_media(path, application);
    }

    fn navigate_still(&mut self, path: &Path, forward: bool, context: &egui::Context) -> bool {
        let Some(index) = self.visible_paths.iter().position(|item| item == path) else {
            return false;
        };
        let candidates = if forward {
            self.visible_paths[index + 1..].to_vec()
        } else {
            self.visible_paths[..index].iter().rev().cloned().collect()
        };
        let next = candidates.into_iter().find(|candidate| {
            files::image_file(candidate)
                && self
                    .entries
                    .iter()
                    .chain(self.tree_cache.values().flatten())
                    .any(|entry| {
                        entry.path == *candidate && !entry.directory && entry.sequence.is_none()
                    })
        });
        if let Some(next) = next {
            self.select(Some(next.clone()), context);
            self.scroll_to_selection = true;
            self.open_media(&next, "player");
        }
        true
    }

    fn open_media(&mut self, path: &Path, application: &str) {
        let executable = if application == "player" {
            let executable = std::env::current_exe()
                .ok()
                .and_then(|executable| {
                    let directory = executable.parent()?;
                    std::iter::once(directory.join("player"))
                        .chain(
                            directory
                                .ancestors()
                                .map(|directory| directory.join("player/target/release/player")),
                        )
                        .find(|candidate| candidate.is_file())
                })
                .or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .map(|directory| directory.join("player/target/release/player"))
                        .filter(|launcher| launcher.is_file())
                });
            let Some(executable) = executable else {
                self.set_message(Some(
                    "Cannot find Player next to Still Files or at player/target/release/player."
                        .into(),
                ));
                return;
            };
            executable
        } else {
            PathBuf::from(application)
        };
        let mut command = std::process::Command::new(executable);
        if application == "player"
            && files::image_file(path)
            && !self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .any(|entry| entry.path == path && entry.sequence.is_some())
        {
            command.arg("--still");
        }
        if matches!(application, "vlc" | "mpv") {
            command.arg("--");
            let frames = self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .find(|entry| entry.path == path)
                .and_then(|entry| entry.sequence.as_ref());
            if let Some(frames) = frames {
                command.args(frames.iter());
            } else {
                command.arg(path);
            }
        } else {
            command.arg(path);
        }
        if let Err(error) = command.spawn().map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }) {
            self.set_message(Some(format!(
                "Cannot open file with {application}: {error}"
            )));
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
            self.set_message(Some(format!("Cannot open Konsole: {error}")));
        }
    }

    fn show_properties(&mut self, path: &Path) {
        let parent = self.external_drag.window();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
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
            self.set_message(Some(format!("Cannot open Caja properties: {error}")));
        } else {
            std::thread::spawn(move || properties::center(parent, &name));
        }
    }

    fn directory_background(&mut self, response: &egui::Response, context: &egui::Context) {
        if response.clicked() || response.secondary_clicked() {
            self.sidebar_focused = false;
            self.entry_menu = None;
            self.select(None, context);
        }
        if let Some(paths) = response.dnd_release_payload::<Vec<PathBuf>>() {
            self.move_paths(&paths, &self.current.clone(), context);
        }
        response.context_menu(|ui| self.directory_menu(ui, context));
    }

    fn directory_menu(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if files::is_trash(&self.current) {
            if self
                .icons
                .button(ui, "Refresh · Ctrl+Shift+R / F5", "view-refresh", true)
                .clicked()
            {
                self.refresh(context);
                ui.close_menu();
            }
            if self
                .icons
                .button(
                    ui,
                    "Empty Trash…",
                    "user-trash",
                    self.operation_job.is_none(),
                )
                .clicked()
            {
                self.confirm_empty_trash = true;
                ui.close_menu();
            }
            return;
        }
        if self
            .icons
            .button(ui, "Open Konsole here", "utilities-terminal", true)
            .clicked()
        {
            self.open_terminal(&self.current.clone());
            ui.close_menu();
        }
        if self
            .icons
            .button(
                ui,
                "New folder… · Ctrl+N",
                "folder-new",
                self.operation_job.is_none(),
            )
            .clicked()
        {
            self.start_new_folder(context);
            ui.close_menu();
        }
        if self
            .icons
            .button(ui, "Bookmark this folder", "bookmark-new", true)
            .clicked()
        {
            self.add_bookmark(self.current.clone());
            ui.close_menu();
        }
        if self
            .icons
            .button(
                ui,
                "Compress this folder to ZIP",
                "package-x-generic",
                self.compression_job.is_none(),
            )
            .clicked()
        {
            self.compress(self.current.clone(), context);
            ui.close_menu();
        }
        if self
            .icons
            .button(ui, "Copy directory path", "edit-copy", true)
            .clicked()
        {
            context.copy_text(self.current.to_string_lossy().into_owned());
            ui.close_menu();
        }
        ui.separator();
        if self
            .icons
            .button(ui, "Refresh · Ctrl+Shift+R / F5", "view-refresh", true)
            .clicked()
        {
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
        if self
            .icons
            .button(ui, "Properties", "document-properties", true)
            .clicked()
        {
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

    fn convert(&mut self, entry: Entry, format: conversion::Format, context: &egui::Context) {
        if self.conversion_job.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        self.conversion_job = Some(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(conversion::convert(&entry, format));
            context.request_repaint();
        });
    }

    fn poll(&mut self, context: &egui::Context) {
        if let Some(result) = self
            .conversion_job
            .as_ref()
            .and_then(|job| job.try_recv().ok())
        {
            self.conversion_job = None;
            self.media_cache.clear();
            self.refresh(context);
            match result {
                Ok(path) => {
                    self.set_message(Some(format!("Conversion created: {}", path.display())))
                }
                Err(error) => self.set_message(Some(format!("Cannot convert: {error}"))),
            }
        }
        if let Some(job) = &mut self.operation_job {
            let mut hidden = Vec::new();
            job.pending_visibility
                .retain(|receiver| match receiver.try_recv() {
                    Ok(paths) => {
                        hidden.extend(paths);
                        true
                    }
                    Err(mpsc::TryRecvError::Empty) => true,
                    Err(mpsc::TryRecvError::Disconnected) => false,
                });
            if !hidden.is_empty() {
                job.visual_changed = true;
                job.hidden_paths.extend(hidden.iter().cloned());
                self.update_operation_visibility(&hidden, false, context);
            }
        }
        for _ in 0..64 {
            let Some(job) = &mut self.operation_job else {
                break;
            };
            match job.receiver.try_recv() {
                Ok(OperationUpdate::Visibility(paths, restoring)) => {
                    job.visual_changed = true;
                    if restoring {
                        job.restored_paths.extend(paths.iter().cloned());
                    } else {
                        job.hidden_paths.extend(paths.iter().cloned());
                    }
                    self.update_operation_visibility(&paths, restoring, context);
                }
                Ok(OperationUpdate::Progress(completed, total, path)) => {
                    job.completed = job.completed_offset + completed;
                    if job.trash_paths.is_empty() {
                        job.total = total;
                    }
                    job.path = path;
                }
                Ok(OperationUpdate::Finished(history, outcome)) => {
                    job.outcome.changed |= outcome.changed;
                    job.outcome.pending_delete.extend(outcome.pending_delete);
                    job.outcome.renames.extend(outcome.renames);
                    job.outcome.copied.extend(outcome.copied);
                    job.outcome.trashed.extend(outcome.trashed);
                    job.outcome.selected = outcome.selected;
                    if let Some(error) = outcome.error {
                        match &mut job.outcome.error {
                            Some(previous) => {
                                previous.push('\n');
                                previous.push_str(&error);
                            }
                            None => job.outcome.error = Some(error),
                        }
                    }
                    if !job.pending_trash.is_empty() {
                        job.completed_offset = job.total - job.pending_trash.len();
                        job.completed = job.completed_offset;
                        job.receiver = Self::run_operation(
                            history,
                            FileOperation::Trash(std::mem::take(&mut job.pending_trash)),
                            context,
                        );
                        continue;
                    }
                    let visual_changed = job.visual_changed;
                    let restored_paths = std::mem::take(&mut job.restored_paths);
                    let mut outcome = std::mem::take(&mut job.outcome);
                    if !job.trash_paths.is_empty() && !outcome.trashed.is_empty() {
                        self.copied_files = std::mem::take(&mut outcome.trashed);
                    }
                    self.pending_delete.append(&mut outcome.pending_delete);
                    let previous_selection = job.previous_selection.take();
                    let preserve_selection = job.preserve_selection;
                    let changed = outcome.changed;
                    self.operations = history;
                    self.operation_job = None;
                    outcome.changed |= visual_changed;
                    self.complete_operation(outcome, preserve_selection, context);
                    if visual_changed {
                        self.preparing_directory = false;
                        self.wait_for_thumbnails = false;
                        self.directory_frame = None;
                    }
                    if !restored_paths.is_empty() {
                        self.select_restored_paths(&restored_paths, context);
                    }
                    if let Some((directory, mut state)) = previous_selection.filter(|_| changed) {
                        if directory == self.current && !self.pending_delete.is_empty() {
                            state
                                .paths
                                .retain(|path| self.pending_delete.contains(path));
                            let selected = state
                                .selected
                                .filter(|path| state.paths.contains(path))
                                .or_else(|| state.paths.iter().min().cloned());
                            if selected.is_some() {
                                self.select(selected, context);
                                self.selection = state.paths;
                                self.selection_anchor = state.anchor;
                                self.last_selected = state.last_selected;
                                self.visual_line = state.visual_line;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let restore = job.visual_changed;
                    self.operation_job = None;
                    self.paste_placement = None;
                    if restore {
                        self.refresh(context);
                    }
                    self.set_message(Some("The file operation stopped unexpectedly.".into()));
                }
            }
        }
        if self.operation_job.is_some() {
            context.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if let Some(result) = self
            .directory_refresh
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.directory_refresh = None;
            match result {
                Ok(mut contents) => {
                    if let Some(order) = self.settings.entry_order.get(&self.current) {
                        order_entries(&mut contents.entries, order);
                    }
                    let counts: HashMap<_, _> = self
                        .entries
                        .iter()
                        .chain(self.tree_cache.values().flatten())
                        .filter_map(|entry| {
                            entry.has_contents.map(|has_contents| {
                                (entry.path.clone(), (entry.item_count, has_contents))
                            })
                        })
                        .collect();
                    self.entries = Arc::new(contents.entries);
                    self.tree_cache
                        .retain(|path, _| !contents.expanded.contains(path));
                    self.tree_cache.extend(
                        contents
                            .tree_cache
                            .into_iter()
                            .filter(|(path, _)| self.expanded.contains(path)),
                    );
                    self.tree_layout = None;
                    self.folder_counts.cancel();
                    self.folder_counts.request(&self.entries);
                    for entries in self.tree_cache.values() {
                        self.folder_counts.request(entries);
                    }
                    for entry in Arc::make_mut(&mut self.entries)
                        .iter_mut()
                        .chain(self.tree_cache.values_mut().flatten())
                    {
                        if let Some((count, has_contents)) = counts.get(&entry.path) {
                            entry.item_count = *count;
                            entry.has_contents = Some(*has_contents);
                        }
                    }
                    self.thumbnails.cancel();
                    self.retain_visible_selection(context);
                }
                Err(error) => {
                    self.entries = Arc::default();
                    self.tree_cache.clear();
                    self.tree_layout = None;
                    self.set_message(Some(format!("Cannot read directory: {error}")));
                }
            }
        }
        if let Some(result) = self
            .media_scan
            .as_ref()
            .and_then(|scan| scan.receiver.try_recv().ok())
        {
            match result {
                Ok(result) => {
                    Arc::make_mut(&mut self.media_directories).extend(result.directories);
                    let Some(scan) = &mut self.media_scan else {
                        return;
                    };
                    let entries = Arc::make_mut(&mut self.entries);
                    let indices: HashMap<_, _> = entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| (entry.path.clone(), index))
                        .collect();
                    for entry in result.entries {
                        scan.paths.insert(entry.path.clone());
                        if let Some(&index) = indices.get(&entry.path) {
                            entries[index] = entry;
                        } else {
                            entries.push(entry);
                        }
                    }
                    if result.complete {
                        entries.retain(|entry| scan.paths.contains(&entry.path));
                    }
                    entries.sort_by_cached_key(|entry| {
                        (entry.name.to_lowercase(), entry.path.clone())
                    });
                    if let Some(order) = self.settings.entry_order.get(&self.current) {
                        order_entries(entries, order);
                    }
                    self.tree_layout = None;
                    if result.complete {
                        self.media_scan = None;
                        if result.unreadable == 0 {
                            self.cache_media_directory(None, MediaLayout::default());
                        }
                        if self.directory_selections.contains_key(&self.current) {
                            self.restore_directory_selection(context);
                        } else {
                            self.retain_visible_selection(context);
                        }
                    }
                    if result.complete && result.unreadable > 0 {
                        self.set_message(Some(format!(
                            "Recursive media: {} unreadable directories skipped.",
                            result.unreadable
                        )));
                    }
                }
                Err(error) => {
                    self.media_scan = None;
                    self.set_message(Some(format!("Cannot scan media: {error}")));
                }
            }
        }
        self.watch_directories(context);
        if let Some(search) = &mut self.search {
            search.poll();
        }
        if let Some(job) = &self.operation_job {
            let paths = job.hidden_paths.clone();
            if !paths.is_empty() {
                self.update_operation_visibility(&paths, false, context);
            }
        }
        self.thumbnails.poll(context);
        self.icons.poll(context);
        self.mounts.poll();
        let counts = self.folder_counts.poll();
        if !counts.is_empty() {
            self.tree_layout = None;
            for entry in Arc::make_mut(&mut self.entries)
                .iter_mut()
                .chain(self.tree_cache.values_mut().flatten())
            {
                if let Some(content) = counts.get(&entry.path) {
                    entry.has_contents = Some(content.has_contents);
                    if let Some(count) = content.item_count {
                        entry.item_count = Some(count);
                    }
                }
            }
        }
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
                    Err(error) => {
                        self.set_message(Some(format!("Cannot read disk usage: {error}")))
                    }
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
            self.media_cache.clear();
            match result {
                Ok(path) => {
                    if path.parent() == Some(self.current.as_path()) {
                        self.refresh(context);
                    }
                    self.set_message(Some(format!("Archive created: {}", path.display())));
                }
                Err(error) => self.set_message(Some(format!("Cannot compress: {error}"))),
            }
        }
    }

    fn watch_directories(&mut self, context: &egui::Context) {
        let Some(watch) = &mut self.directory_watch else {
            return;
        };
        let mut paths = HashSet::new();
        if !files::is_trash(&self.current) {
            paths.insert(self.current.clone());
            if self.settings.recursive() {
                paths.extend(
                    self.media_directories
                        .iter()
                        .map(|directory| directory.path.clone()),
                );
            } else if self.settings.view == View::Tree {
                paths.extend(self.expanded.iter().cloned());
            }
        }
        watch.synchronize(&self.current, paths, self.media_scan.is_some());
        let busy = self.operation_job.is_some()
            || self.new_folder.is_some()
            || self.directory_refresh.is_some()
            || self.media_scan.is_some()
            || self.compression_job.is_some()
            || self.conversion_job.is_some();
        if watch.ready(context, busy) {
            self.refresh(context);
        }
    }

    fn pane_shortcuts(&mut self, context: &egui::Context) {
        if self.new_folder.is_some()
            || context.memory(|memory| memory.has_focus(egui::Id::new("path-input")))
        {
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
            self.pane_selection_y = if self.sidebar_focused && !sidebar {
                self.sidebar_cursor_y
            } else {
                None
            };
            self.sidebar_focused = sidebar;
            self.motion_count = 0;
            self.pending_g = false;
            self.pending_z = false;
            self.pending_y = false;
            self.pending_d = false;
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
            self.pending_z = false;
            self.pending_y = false;
            self.pending_d = false;
            return;
        }
        let mut directory_steps = Vec::new();
        context.input_mut(|input| {
            input.events.retain_mut(|event| {
                if matches!(
                    event,
                    egui::Event::PointerButton { pressed: true, .. }
                        | egui::Event::WindowFocused(false)
                ) {
                    self.pending_g = false;
                    self.pending_z = false;
                    self.pending_y = false;
                    self.pending_d = false;
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
                    return !(matches!(key, Key::G | Key::Z | Key::Y)
                        || *key == Key::D && modifiers.matches_exact(egui::Modifiers::NONE));
                }
                let pending_y = std::mem::take(&mut self.pending_y);
                let pending_z = std::mem::take(&mut self.pending_z);
                let pending_d = std::mem::take(&mut self.pending_d);
                if *key == Key::D && modifiers.matches_exact(egui::Modifiers::NONE) {
                    self.pending_g = false;
                    if pending_d || self.visual_line && !self.sidebar_focused {
                        return true;
                    }
                    self.pending_d = true;
                    return false;
                }
                if *key == Key::Y
                    && modifiers.matches_exact(egui::Modifiers::NONE)
                    && !self.sidebar_focused
                {
                    self.pending_g = false;
                    if self.visual_line || pending_y {
                        return true;
                    }
                    self.pending_y = true;
                    return false;
                }
                if *key == Key::Z
                    && modifiers.matches_exact(egui::Modifiers::NONE)
                    && !self.sidebar_focused
                {
                    self.pending_g = false;
                    if pending_z {
                        if self.selected.is_some() {
                            self.center_selection = true;
                            self.scroll_to_selection = true;
                        }
                    } else {
                        self.pending_z = true;
                    }
                    return false;
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
                    directory_steps.push(!modifiers.shift);
                    self.pending_g = false;
                    return false;
                }
                self.pending_g = false;
                true
            });
        });
        for forward in directory_steps {
            let next = if forward {
                self.breadcrumb_path
                    .ancestors()
                    .find(|path| path.parent() == Some(self.current.as_path()))
                    .map(Path::to_path_buf)
            } else {
                self.current.parent().map(Path::to_path_buf)
            };
            if let Some(path) = next {
                self.navigate_breadcrumb(path, context);
            }
        }
    }

    fn visual_line_shortcut(&mut self, context: &egui::Context) -> bool {
        let enabled = !self.sidebar_focused
            && self.search.is_none()
            && self.new_folder.is_none()
            && !self.text_input_focused(context);
        let mut toggle = false;
        context.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key: Key::V,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } if enabled
                    && (modifiers.matches_exact(egui::Modifiers::NONE)
                        || modifiers.matches_exact(egui::Modifiers::SHIFT)) =>
                {
                    toggle |= !repeat;
                    false
                }
                _ => true,
            });
        });
        toggle
    }

    fn shortcuts(&mut self, context: &egui::Context) {
        if self.new_folder.is_none()
            && !self.sidebar_focused
            && !self.text_input_focused(context)
            && context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::O))
        {
            if let Some(entry) = self.selected.as_ref().and_then(|path| {
                self.entries
                    .iter()
                    .chain(self.tree_cache.values().flatten())
                    .find(|entry| &entry.path == path)
                    .cloned()
            }) {
                let position = self
                    .prepared_view
                    .iter()
                    .find(|(path, _)| path == &entry.path)
                    .map_or(context.screen_rect().center(), |(_, rect)| {
                        rect.left_bottom()
                    });
                self.open_entry_menu(entry, position, true);
            }
            return;
        }
        self.pane_shortcuts(context);
        if self.new_folder.is_none() {
            if !self.text_input_focused(context)
                && context.input_mut(|input| {
                    input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::R)
                })
            {
                self.refresh(context);
            }
            if !self.text_input_focused(context)
                && context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::M))
            {
                let view = if self.settings.view == View::Media {
                    match self.settings.media_return_view {
                        View::Tree => View::Tree,
                        _ => View::Grid,
                    }
                } else {
                    View::Media
                };
                self.set_view(view, context);
            }
            if !self.text_input_focused(context)
                && context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::R))
            {
                *self.settings.recursive_mut() = !self.settings.recursive();
                self.refresh(context);
                self.save();
            }
            if !self.text_input_focused(context)
                && context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::S))
            {
                self.settings.sequences = !self.settings.sequences;
                self.refresh(context);
                self.save();
            }
        }
        if self.new_folder.is_none() && !self.sidebar_focused {
            if let Some(search) = &mut self.search {
                for (key, forward) in [
                    (Key::H, false),
                    (Key::J, true),
                    (Key::K, false),
                    (Key::L, true),
                    (Key::P, false),
                    (Key::N, true),
                ] {
                    if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, key)) {
                        if let Some(update) = &search.update {
                            search.cursor = if forward {
                                search
                                    .cursor
                                    .saturating_add(1)
                                    .min(update.hits.len().saturating_sub(1))
                            } else {
                                search.cursor.saturating_sub(1)
                            };
                            search.reveal_cursor = true;
                        }
                    }
                }
            }
        }
        self.vim_shortcuts(context);
        if self.new_folder.is_none() && !self.text_input_focused(context) {
            for (modifiers, key, view) in [
                (egui::Modifiers::NONE, Key::T, View::Tree),
                (egui::Modifiers::CTRL, Key::G, View::Grid),
            ] {
                if context.input_mut(|input| input.consume_key(modifiers, key))
                    && self.settings.view != view
                {
                    self.set_view(view, context);
                }
            }
        }
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
            let zoom = (self.settings.zoom() * (scroll * 0.003).exp()).clamp(0.65, 3.6);
            if self.settings.zoom() != zoom {
                *self.settings.zoom_mut() = zoom;
                self.save();
            }
        }
        if self.new_folder.is_none()
            && context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::F))
        {
            self.search = None;
            self.editing_path = false;
            self.sidebar_focused = false;
            self.find_query.get_or_insert_with(String::new);
            self.focus_find = true;
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::P)) {
            self.start_search(context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::L)) {
            self.toggle_path_field(context);
        }
        if self.new_folder.is_none()
            && self.search.is_none()
            && !self.text_input_focused(context)
            && context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::N))
        {
            self.start_new_folder(context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if self.new_folder.is_some() {
                self.cancel_name_edit(context);
            } else if self.message.is_some() {
                self.message = None;
            } else if self.find_query.is_some() {
                self.find_query = None;
                context.memory_mut(|memory| memory.surrender_focus(egui::Id::new("find-input")));
            } else if self.search.is_some() {
                self.search = None;
                context.memory_mut(|memory| memory.surrender_focus(egui::Id::new("search-input")));
            } else if self.editing_path {
                self.editing_path = false;
            } else if self.visual_line {
                self.exit_visual(context);
            } else {
                self.select(None, context);
            }
        }
        if self.new_folder.is_none() && !self.text_input_focused(context) {
            for (key, delta) in [(Key::Equals, 0.1), (Key::Minus, -0.1)] {
                if context.input_mut(|input| {
                    input.modifiers.matches_exact(egui::Modifiers::NONE)
                        && input.consume_key(egui::Modifiers::NONE, key)
                }) {
                    let zoom = (self.settings.zoom() + delta).clamp(0.65, 3.6);
                    if self.settings.zoom() != zoom {
                        *self.settings.zoom_mut() = zoom;
                        self.save();
                    }
                }
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::U)) {
                if self.sidebar_focused {
                    self.bookmark_history_step(false);
                } else {
                    self.start_operation(FileOperation::Undo, context);
                }
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::R)) {
                if self.sidebar_focused {
                    self.bookmark_history_step(true);
                } else {
                    self.start_operation(FileOperation::Redo, context);
                }
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
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::H)) {
                self.settings.hidden = !self.settings.hidden;
                self.refresh(context);
                self.save();
            }
        }
        if self.sidebar_focused && self.new_folder.is_none() && !self.text_input_focused(context) {
            return;
        }
        if self.search.is_some() && self.new_folder.is_none() {
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::F5)) {
                self.refresh(context);
            }
            if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::H)) {
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
            if self.visual_line {
                self.exit_visual(context);
            } else {
                let path = self
                    .selected
                    .clone()
                    .or_else(|| self.visible_paths.first().cloned());
                self.select(path, context);
                self.visual_line = self.selected.is_some();
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Y)) {
            self.yank();
        }
        if self.settings.view == View::Tree
            && context.input_mut(|input| input.consume_key(egui::Modifiers::ALT, Key::P))
        {
            self.motion_count = 0;
            self.pending_g = false;
            if let Some(parent) = self.selected.as_ref().and_then(|path| path.parent()) {
                if self.visible_paths.iter().any(|path| path == parent) {
                    self.select_entry(parent.to_path_buf(), context);
                    self.scroll_to_selection = true;
                }
            }
            return;
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::P)) {
            self.paste(context);
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::D)) {
            self.trash_selection(context);
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
            self.visual_line = false;
            let paths: Vec<_> = self
                .selection
                .iter()
                .filter(|path| {
                    path.is_dir()
                        && !path.is_symlink()
                        && !path
                            .ancestors()
                            .skip(1)
                            .any(|parent| self.selection.contains(parent))
                })
                .cloned()
                .collect();
            for path in paths {
                self.toggle_recursive_expansion(path);
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::O)) {
            self.visual_line = false;
            if self.settings.view == View::Tree {
                for path in self.selection.clone() {
                    if path.is_dir() && !path.is_symlink() && !self.expanded.remove(&path) {
                        self.expand_directory(&path);
                    }
                }
            }
            if let Some(path) = self.selected.clone() {
                if self.settings.view != View::Tree || !path.is_dir() || path.is_symlink() {
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
        self.find_query.is_some()
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

    fn collect_motion_count(&mut self, context: &egui::Context) {
        context.input_mut(|input| {
            input.events.retain(|event| {
                if let egui::Event::Text(text) = event {
                    if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
                        for digit in text.bytes() {
                            self.motion_count = self
                                .motion_count
                                .saturating_mul(10)
                                .saturating_add((digit - b'0') as usize);
                        }
                        return false;
                    }
                }
                true
            });
        });
    }

    fn number_width(&self) -> f32 {
        28.0
    }

    fn paint_relative_number(&self, ui: &egui::Ui, rect: egui::Rect, row: usize, current: usize) {
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        let gap = if self.settings.view == View::Grid {
            ui.spacing().item_spacing.y
        } else {
            0.0
        };
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom() + Vec2::new(0.0, gap)],
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if row == current {
                row + 1
            } else {
                row.abs_diff(current)
            }
            .to_string(),
            egui::FontId::monospace(10.0),
            if row == current {
                visual_accent(ui.visuals().dark_mode)
            } else {
                MUTED
            },
        );
    }

    fn navigate_items(
        &mut self,
        items: &[(&Entry, egui::Pos2, f32)],
        ui: &egui::Ui,
        scroll_id: &str,
        content_height: f32,
        context: &egui::Context,
    ) {
        if !self
            .visible_paths
            .iter()
            .eq(items.iter().map(|(entry, _, _)| &entry.path))
        {
            self.visible_paths = items
                .iter()
                .map(|(entry, _, _)| entry.path.clone())
                .collect();
        }
        if self.sidebar_focused {
            return;
        }
        if self.search.is_some()
            || self.new_folder.is_some()
            || self.confirm_empty_trash
            || !self.pending_delete.is_empty()
            || self.text_input_focused(context)
            || items.is_empty()
        {
            return;
        }
        if let Some(y) = self.pane_selection_y.take() {
            let offset = egui::scroll_area::State::load(
                context,
                ui.make_persistent_id(egui::Id::new(scroll_id)),
            )
            .map_or(0.0, |state| state.offset.y);
            let top = ui.available_rect_before_wrap().top();
            let bottom = top + ui.available_height();
            if let Some((entry, _, _)) = items
                .iter()
                .filter(|(_, position, height)| {
                    position.y + height > offset && position.y < offset + bottom - top
                })
                .min_by(|(_, a, a_height), (_, b, b_height)| {
                    (top + a.y + a_height / 2.0 - offset - y)
                        .abs()
                        .total_cmp(&(top + b.y + b_height / 2.0 - offset - y).abs())
                        .then_with(|| a.x.total_cmp(&b.x))
                })
            {
                self.select(Some(entry.path.clone()), context);
                self.scroll_to_selection = false;
                self.center_selection = false;
            }
        }
        self.collect_motion_count(context);
        if context.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::A)) {
            if !self
                .selected
                .as_ref()
                .is_some_and(|path| self.visible_paths.contains(path))
            {
                self.select(self.visible_paths.first().cloned(), context);
            }
            self.pending_g = false;
            self.selection = self.visible_paths.iter().cloned().collect();
            if self.selection.len() > 1 {
                self.size_job = None;
                self.selection_size = None;
            }
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
            let viewport_height = ui.available_height();
            let offset = egui::scroll_area::State::load(
                context,
                ui.make_persistent_id(egui::Id::new(scroll_id)),
            )
            .map_or(0.0, |state| state.offset.y);
            let next_offset = (offset + page_direction * viewport_height / 2.0)
                .clamp(0.0, (content_height - viewport_height).max(0.0));
            self.page_scroll_delta = next_offset - offset;
            let x = items
                .iter()
                .find(|(entry, _, _)| self.selected.as_ref() == Some(&entry.path))
                .unwrap_or(&items[0])
                .1
                .x;
            let bottom = next_offset + viewport_height;
            let edge_distance = |position: egui::Pos2, height: f32| {
                if page_direction > 0.0 {
                    (bottom - position.y - height).abs()
                } else {
                    (position.y - next_offset).abs()
                }
            };
            let closest = |fully_visible: bool| {
                items
                    .iter()
                    .filter(|(_, position, height)| {
                        if fully_visible {
                            position.y >= next_offset - 0.1 && position.y + height <= bottom + 0.1
                        } else {
                            position.y < bottom && position.y + height > next_offset
                        }
                    })
                    .min_by(|(_, a, a_height), (_, b, b_height)| {
                        edge_distance(*a, *a_height)
                            .total_cmp(&edge_distance(*b, *b_height))
                            .then_with(|| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
                    })
            };
            if let Some((entry, position, height)) = closest(true).or_else(|| closest(false)) {
                let aligned_offset = if page_direction > 0.0 {
                    position.y + height - viewport_height
                } else {
                    position.y
                };
                self.page_scroll_delta =
                    aligned_offset.clamp(0.0, (content_height - viewport_height).max(0.0)) - offset;
                self.select_entry(entry.path.clone(), context);
            }
            self.scroll_to_selection = false;
            return;
        }
        if matches!(self.settings.view, View::Media | View::Grid) {
            let motion = context.input_mut(|input| {
                let mut row_start = false;
                input.events.retain(|event| {
                    if matches!(event, egui::Event::Text(text) if text == "_") {
                        row_start = true;
                        return false;
                    }
                    true
                });
                if row_start {
                    return Some(Key::Minus);
                }
                [Key::W, Key::E, Key::B, Key::Minus]
                    .into_iter()
                    .find(|key| input.consume_key(egui::Modifiers::NONE, *key))
                    .or_else(|| {
                        input
                            .consume_key(egui::Modifiers::SHIFT, Key::Minus)
                            .then_some(Key::Minus)
                    })
                    .or_else(|| {
                        input
                            .consume_key(egui::Modifiers::SHIFT, Key::Num4)
                            .then_some(Key::Num4)
                    })
            });
            if let Some(key) = motion {
                let current = items
                    .iter()
                    .position(|(entry, _, _)| self.selected.as_ref() == Some(&entry.path));
                let index = current.unwrap_or(0);
                let count = std::mem::take(&mut self.motion_count)
                    .max(1)
                    .min(items.len());
                let target = match key {
                    Key::W | Key::E if current.is_some() => {
                        index.saturating_add(count).min(items.len() - 1)
                    }
                    Key::B => index.saturating_sub(count),
                    Key::Minus => items
                        .iter()
                        .position(|(_, position, _)| position.y == items[index].1.y)
                        .unwrap_or(index),
                    Key::Num4 => items
                        .iter()
                        .rposition(|(_, position, _)| position.y == items[index].1.y)
                        .unwrap_or(index),
                    _ => index,
                };
                let path = items[target].0.path.clone();
                if self.visual_line {
                    self.select_entry(path, context);
                } else {
                    self.select(Some(path), context);
                }
                self.scroll_to_selection = true;
                return;
            }
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
            .position(|(entry, _, _)| self.selected.as_ref() == Some(&entry.path));
        let count = std::mem::take(&mut self.motion_count)
            .max(1)
            .min(items.len());
        if matches!(key, Key::ArrowUp | Key::ArrowDown) {
            if let Some(index) = current {
                let position = items[index].1;
                let mut rows: Vec<_> = items.iter().map(|(_, position, _)| position.y).collect();
                rows.dedup();
                let current_row = rows.iter().position(|y| *y == position.y).unwrap_or(0);
                let target_row = if key == Key::ArrowUp {
                    current_row.saturating_sub(count)
                } else {
                    current_row.saturating_add(count).min(rows.len() - 1)
                };
                if let Some((entry, _, _)) = items
                    .iter()
                    .filter(|(_, candidate, _)| candidate.y == rows[target_row])
                    .min_by(|(_, a, _), (_, b, _)| {
                        (a.x - position.x)
                            .abs()
                            .total_cmp(&(b.x - position.x).abs())
                    })
                {
                    self.select_entry(entry.path.clone(), context);
                    self.scroll_to_selection = true;
                }
                return;
            }
        }
        let target = if let Some(index) = current {
            let (entry, position, _) = &items[index];
            if self.settings.view == View::Tree {
                match key {
                    Key::ArrowRight if entry.directory && !entry.symlink => {
                        if !self.expanded.contains(&entry.path) {
                            self.expand_directory(&entry.path);
                            return;
                        }
                        items.get(index + 1).filter(|(child, _, _)| {
                            child.path.parent() == Some(entry.path.as_path())
                        })
                    }
                    Key::ArrowLeft => {
                        if self.expanded.remove(&entry.path) {
                            return;
                        }
                        items.iter().find(|(parent, _, _)| {
                            entry.path.parent() == Some(parent.path.as_path())
                        })
                    }
                    Key::ArrowUp => index.checked_sub(1).and_then(|index| items.get(index)),
                    Key::ArrowDown => items.get(index + 1),
                    _ => None,
                }
            } else {
                items
                    .iter()
                    .filter(|(_, candidate, _)| match key {
                        Key::ArrowUp => candidate.y < position.y,
                        Key::ArrowDown => candidate.y > position.y,
                        Key::ArrowLeft => candidate.y == position.y && candidate.x < position.x,
                        Key::ArrowRight => candidate.y == position.y && candidate.x > position.x,
                        _ => false,
                    })
                    .min_by(|(_, a, _), (_, b, _)| {
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
                .find(|(entry, _, _)| self.last_selected.as_ref() == Some(&entry.path))
                .or_else(|| {
                    if matches!(key, Key::ArrowUp | Key::ArrowLeft) {
                        items.last()
                    } else {
                        items.first()
                    }
                })
        };
        if let Some((entry, _, _)) = target {
            self.select_entry(entry.path.clone(), context);
            self.scroll_to_selection = true;
        }
    }

    fn entry_scroll_area(&mut self, ui: &egui::Ui, id: &str) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical().id_salt(id);
        let delta = std::mem::take(&mut self.page_scroll_delta);
        if delta != 0.0 {
            let offset =
                egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id(egui::Id::new(id)))
                    .map_or(0.0, |state| state.offset.y);
            scroll = scroll.vertical_scroll_offset((offset + delta).max(0.0));
        }
        scroll
    }

    fn expand_directory(&mut self, path: &Path) {
        match files::entries(path, self.settings.hidden, self.settings.sequences) {
            Ok(mut children) => {
                self.folder_counts.restore(&mut children);
                self.tree_layout = None;
                self.folder_counts.request(&children);
                self.tree_cache.insert(path.to_path_buf(), children);
                self.expanded.insert(path.to_path_buf());
            }
            Err(error) => self.set_message(Some(format!("Cannot expand directory: {error}"))),
        }
    }

    fn toggle_recursive_expansion(&mut self, path: PathBuf) {
        self.tree_layout = None;
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
                Ok(mut children) => {
                    self.folder_counts.restore(&mut children);
                    pending.extend(
                        children
                            .iter()
                            .filter(|entry| entry.directory && !entry.symlink)
                            .map(|entry| (entry.path.clone(), depth + 1)),
                    );
                    self.folder_counts.request(&children);
                    self.tree_cache.insert(path.clone(), children);
                    self.expanded.insert(path);
                    expanded += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    limited = true;
                    break;
                }
                Err(error) => self.set_message(Some(format!("Cannot expand directory: {error}"))),
            }
        }
        if limited {
            self.set_message(Some(format!(
                "Recursive expansion limited to {RECURSIVE_MAX_DEPTH} levels, \
                 {RECURSIVE_MAX_FOLDERS} folders, {RECURSIVE_MAX_ENTRIES} scanned entries \
                 or {} ms. Open remaining folders manually.",
                RECURSIVE_MAX_TIME.as_millis()
            )));
        }
    }

    fn toolbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(8, 8)),
            )
            .show(context, |ui| {
                ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
                let (row, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 32.0),
                    egui::Sense::hover(),
                );
                let navigation =
                    egui::Rect::from_min_max(row.min, egui::pos2(row.left() + 176.0, row.bottom()));
                let options =
                    egui::Rect::from_min_max(egui::pos2(row.right() - 344.0, row.top()), row.max);
                let address = egui::Rect::from_min_max(
                    egui::pos2(navigation.right() + 16.0, row.top()),
                    egui::pos2(options.left() - 16.0, row.bottom()),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(navigation)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        if arrow_button(ui, self.position > 0, Vec2::new(-1.0, 0.0)).clicked() {
                            self.history_step(false, context);
                        }
                        if arrow_button(
                            ui,
                            self.position + 1 < self.history.len(),
                            Vec2::new(1.0, 0.0),
                        )
                        .clicked()
                        {
                            self.history_step(true, context);
                        }
                        if arrow_button(ui, self.current.parent().is_some(), Vec2::new(0.0, -1.0))
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
                            .clicked()
                        {
                            self.toggle_path_field(context);
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
                        if theme_button(ui).clicked() {
                            self.settings.dark_mode = !self.settings.dark_mode;
                            apply_theme(context, self.settings.dark_mode);
                            self.save();
                        }
                        if hidden_button(ui, self.settings.hidden).clicked() {
                            self.settings.hidden = !self.settings.hidden;
                            self.refresh(context);
                            self.save();
                        }
                        if sequences_button(ui, self.settings.sequences).clicked() {
                            self.settings.sequences = !self.settings.sequences;
                            self.refresh(context);
                            self.save();
                        }
                        if recursive_media_button(ui, self.settings.recursive()).clicked() {
                            *self.settings.recursive_mut() = !self.settings.recursive();
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
                                self.set_view(view, context);
                            }
                        }
                    },
                );
            });
    }

    fn toggle_path_field(&mut self, context: &egui::Context) {
        self.find_query = None;
        self.search = None;
        self.address = self.current.to_string_lossy().into_owned();
        self.sidebar_focused = false;
        self.editing_path = !self.editing_path;
        self.focus_path = self.editing_path;
        if !self.editing_path {
            context.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
    }

    fn start_search(&mut self, context: &egui::Context) {
        if files::is_trash(&self.current) {
            self.find_query.get_or_insert_with(String::new);
            self.focus_find = true;
            self.sidebar_focused = false;
            return;
        }
        self.find_query = None;
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

    fn reveal_search_result(&mut self, path: PathBuf, context: &egui::Context) {
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let entries = match files::entries(&parent, self.settings.hidden, self.settings.sequences) {
            Ok(entries) => entries,
            Err(error) => {
                self.set_message(Some(format!("Cannot open directory: {error}")));
                return;
            }
        };
        let selected = entries
            .iter()
            .find(|entry| {
                entry.path == path
                    || entry
                        .sequence
                        .as_ref()
                        .is_some_and(|frames| frames.contains(&path))
            })
            .map(|entry| entry.path.clone());
        self.search = None;
        self.sidebar_focused = false;
        if self.settings.view == View::Media
            && !path.is_dir()
            && !files::image_file(&path)
            && !files::video_file(&path)
        {
            self.settings.set_view(View::Grid);
        }
        *self.settings.recursive_mut() = false;
        self.thumbnails.cancel();
        if parent != self.current {
            self.history.truncate(self.position + 1);
            self.history.push(parent.clone());
            self.position = self.history.len() - 1;
        }
        self.breadcrumb_path = parent.clone();
        self.load_directory(parent, entries, context);
        self.select(selected, context);
        self.scroll_to_selection = true;
        self.center_selection = true;
        self.refresh_disk(context);
        self.save();
    }

    fn address_bar(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if let Some(query) = &mut self.find_query {
            let response = ui.add_sized(
                [ui.available_width(), 32.0],
                egui::TextEdit::singleline(query)
                    .id(egui::Id::new("find-input"))
                    .vertical_align(egui::Align::Center)
                    .hint_text("Find and select visible items…"),
            );
            if self.focus_find {
                response.request_focus();
                self.focus_find = false;
            }
            if response.changed() {
                let query = query.to_lowercase();
                let best = self
                    .visible_paths
                    .iter()
                    .filter_map(|path| {
                        let name = path.file_name()?.to_string_lossy().to_lowercase();
                        search::selection_score(&name, &query).map(|score| (path, score))
                    })
                    .min_by_key(|(_, score)| *score)
                    .map(|(path, _)| path.clone());
                if best.is_some() {
                    if best != self.selected {
                        self.select(best, context);
                    }
                    self.center_selection = true;
                    self.scroll_to_selection = true;
                }
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                self.find_query = None;
                response.surrender_focus();
            }
        } else if let Some(search) = &mut self.search {
            let response = ui.add_sized(
                [ui.available_width(), 32.0],
                egui::TextEdit::singleline(&mut search.query)
                    .id(egui::Id::new("search-input"))
                    .vertical_align(egui::Align::Center)
                    .hint_text("Search this folder and subfolders…"),
            );
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
                self.reveal_search_result(path, context);
            }
        } else if self.editing_path {
            let id = egui::Id::new("path-input");
            if self.focus_path {
                self.path_completion =
                    PathCompletion::new(&self.address, &self.current, self.settings.hidden);
            }
            let mut completed = false;
            if context.memory(|memory| memory.has_focus(id)) {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::W)) {
                    let trimmed = self.address.trim_end_matches('/');
                    let end = trimmed
                        .rfind('/')
                        .map_or(0, |index| if index == 0 { 1 } else { index });
                    let end = if trimmed.is_empty() && self.address.starts_with('/') {
                        1
                    } else {
                        end
                    };
                    self.address.truncate(end);
                    self.path_completion =
                        PathCompletion::new(&self.address, &self.current, self.settings.hidden);
                    completed = true;
                }
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, Key::E)) {
                    let at_end = egui::TextEdit::load_state(context, id)
                        .and_then(|state| state.cursor.char_range())
                        .is_some_and(|range| {
                            range.primary.index == range.secondary.index
                                && range.primary.index == self.address.chars().count()
                        });
                    if at_end {
                        if let Some(candidate) = self
                            .path_completion
                            .candidates
                            .first()
                            .filter(|candidate| candidate.starts_with(&self.address))
                        {
                            self.address = candidate.clone();
                            self.path_completion = PathCompletion::new(
                                &self.address,
                                &self.current,
                                self.settings.hidden,
                            );
                            completed = true;
                        }
                    }
                }
                let backward =
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, Key::Tab));
                let forward =
                    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Tab));
                if backward || forward {
                    if let Some(candidate) = self.path_completion.cycle(backward) {
                        self.address = candidate.to_owned();
                        completed = true;
                    }
                }
            }
            let output = egui::TextEdit::singleline(&mut self.address)
                .id(id)
                .lock_focus(true)
                .desired_width(ui.available_width())
                .min_size(Vec2::new(0.0, 32.0))
                .vertical_align(egui::Align::Center)
                .show(ui);
            let response = &output.response;
            if response.changed() {
                self.path_completion =
                    PathCompletion::new(&self.address, &self.current, self.settings.hidden);
            }
            if response.has_focus()
                && output.cursor_range.is_some_and(|range| {
                    range.is_empty() && range.primary.ccursor.index == self.address.chars().count()
                })
            {
                if let Some(suffix) = self
                    .path_completion
                    .candidates
                    .first()
                    .and_then(|candidate| candidate.strip_prefix(&self.address))
                {
                    ui.painter().with_clip_rect(output.text_clip_rect).text(
                        output.galley_pos + Vec2::new(output.galley.size().x, 0.0),
                        egui::Align2::LEFT_TOP,
                        suffix,
                        egui::TextStyle::Body.resolve(ui.style()),
                        ui.visuals().weak_text_color(),
                    );
                }
            }
            if completed {
                let mut state = output.state;
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(self.address.chars().count()),
                    )));
                state.store(context, id);
            }
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
                let path = if path.is_absolute() || files::is_trash(&path) {
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
                        if files::is_trash(&self.current) {
                            ui.add(
                                egui::Button::new("Trash")
                                    .selected(true)
                                    .min_size(Vec2::new(0.0, 32.0)),
                            );
                            return;
                        }
                        let trail = self.breadcrumb_path.clone();
                        let mut path = PathBuf::new();
                        for component in trail.components() {
                            path.push(component);
                            let label = component.as_os_str().to_string_lossy();
                            let response = ui.add(
                                egui::Button::new(label.as_ref())
                                    .selected(path == self.current)
                                    .min_size(Vec2::new(0.0, 32.0)),
                            );
                            if response.clicked() {
                                self.navigate_breadcrumb(path.clone(), context);
                            }
                            if response.secondary_clicked() {
                                let metadata = std::fs::symlink_metadata(&path).ok();
                                self.open_entry_menu(
                                    Entry {
                                        path: path.clone(),
                                        name: label.into_owned(),
                                        directory: true,
                                        symlink: metadata
                                            .as_ref()
                                            .is_some_and(|metadata| metadata.is_symlink()),
                                        deletion_locked: files::deletion_locked(&path),
                                        size: metadata
                                            .as_ref()
                                            .map_or(0, |metadata| metadata.len()),
                                        item_count: None,
                                        has_contents: None,
                                        modified: metadata
                                            .and_then(|metadata| metadata.modified().ok()),
                                        modified_label: String::new(),
                                        sequence: None,
                                    },
                                    response
                                        .interact_pointer_pos()
                                        .unwrap_or(response.rect.center()),
                                    false,
                                );
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
        let gutter =
            egui::Rect::from_min_size(rect.min, Vec2::new(self.number_width(), rect.height()));
        self.paint_relative_number(ui, gutter, index, self.sidebar_cursor.unwrap_or(0));
        let rect = egui::Rect::from_min_max(gutter.right_top() + Vec2::new(8.0, 0.0), rect.max);
        let current = bookmark.path.as_ref() == Some(&self.current)
            || (bookmark.uri == "trash:///" && files::is_trash(&self.current));
        if current {
            ui.painter().rect_filled(
                rect.shrink(2.0),
                5.0,
                Color32::from_rgba_unmultiplied(128, 128, 128, 100),
            );
        }
        if self.sidebar_cursor == Some(index) {
            self.sidebar_cursor_y = Some(rect.center().y);
        }
        if self.sidebar_focused && self.sidebar_cursor == Some(index) {
            ui.painter().rect_stroke(
                rect.shrink(2.0),
                5.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
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
        let location = bookmark.path.as_deref().map_or_else(
            || {
                url::Url::parse(&bookmark.uri)
                    .ok()
                    .filter(|uri| !matches!(uri.scheme(), "file" | "trash"))
                    .map(|_| mounts::Kind::Network)
            },
            |path| self.mounts.kind(path),
        );
        paint_folder_location(ui, icon, location);
        paint_name(
            ui,
            &bookmark.label,
            egui::pos2(rect.left() + 34.0, rect.center().y),
            rect.width() - 40.0,
            13.0,
            ui.visuals().text_color(),
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
            if matches!(kind, Some(places::Kind::Trash)) {
                if self
                    .icons
                    .button(
                        ui,
                        "Empty Trash…",
                        "user-trash",
                        self.operation_job.is_none(),
                    )
                    .clicked()
                {
                    self.confirm_empty_trash = true;
                    ui.close_menu();
                }
                return;
            }
            if self
                .icons
                .button(ui, "Remove bookmark", "bookmark-remove", true)
                .clicked()
            {
                self.remove_bookmark(bookmark, kind);
                ui.close_menu();
            }
        });
    }

    fn set_bookmark_removed(
        &mut self,
        bookmark: &bookmarks::Bookmark,
        kind: Option<places::Kind>,
        index: usize,
        removed: bool,
    ) -> std::io::Result<()> {
        if let Some(path) = bookmark.path.as_ref().filter(|_| kind.is_some()) {
            self.settings.removed_places.retain(|entry| entry != path);
            if removed {
                self.settings.removed_places.push(path.clone());
            }
            self.save();
            Ok(())
        } else if removed {
            self.bookmarks.remove(&bookmark.uri)
        } else {
            self.bookmarks.restore(bookmark, index)
        }
    }

    fn remove_bookmark(&mut self, bookmark: &bookmarks::Bookmark, kind: Option<places::Kind>) {
        if matches!(kind, Some(places::Kind::Trash)) {
            return;
        }
        let index = self
            .bookmarks
            .entries
            .iter()
            .position(|entry| entry.uri == bookmark.uri)
            .unwrap_or(0);
        match self.set_bookmark_removed(bookmark, kind, index, true) {
            Ok(()) => {
                self.bookmark_undo.push((bookmark.clone(), kind, index));
                self.bookmark_redo.clear();
            }
            Err(error) => {
                self.set_message(Some(format!("Cannot save shared bookmarks: {error}")));
            }
        }
    }

    fn bookmark_history_step(&mut self, redo: bool) {
        let source = if redo {
            &mut self.bookmark_redo
        } else {
            &mut self.bookmark_undo
        };
        let Some((bookmark, kind, index)) = source.pop() else {
            return;
        };
        let result = self.set_bookmark_removed(&bookmark, kind, index, redo);
        let destination = if redo == result.is_ok() {
            &mut self.bookmark_undo
        } else {
            &mut self.bookmark_redo
        };
        destination.push((bookmark, kind, index));
        if let Err(error) = result {
            self.set_message(Some(format!("Cannot save shared bookmarks: {error}")));
        }
    }

    fn open_bookmark(&mut self, bookmark: &bookmarks::Bookmark, context: &egui::Context) {
        if bookmark.uri == "trash:///" {
            self.navigate(PathBuf::from("trash:///"), context);
        } else if let Some(path) = &bookmark.path {
            self.navigate(path.clone(), context);
        } else if let Err(error) = std::process::Command::new("xdg-open")
            .arg(&bookmark.uri)
            .spawn()
        {
            self.set_message(Some(format!("Cannot open bookmark: {error}")));
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
        entries.push((
            bookmarks::Bookmark {
                uri: "trash:///".into(),
                path: None,
                label: "Trash".into(),
                exists: true,
            },
            Some(places::Kind::Trash),
        ));
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
        let mut cursor = self
            .sidebar_cursor
            .unwrap_or_else(|| {
                entries
                    .iter()
                    .position(|(bookmark, _)| bookmark.path.as_ref() == Some(&self.current))
                    .unwrap_or(0)
            })
            .min(entries.len().saturating_sub(1));
        if self.sidebar_focused
            && self.new_folder.is_none()
            && !self.confirm_empty_trash
            && self.pending_delete.is_empty()
            && !self.text_input_focused(context)
        {
            self.collect_motion_count(context);
            let previous = cursor;
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                    || input.consume_key(egui::Modifiers::NONE, Key::J)
            }) {
                let count = std::mem::take(&mut self.motion_count).max(1);
                cursor = cursor
                    .saturating_add(count)
                    .min(entries.len().saturating_sub(1));
            }
            if context.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                    || input.consume_key(egui::Modifiers::NONE, Key::K)
            }) {
                let count = std::mem::take(&mut self.motion_count).max(1);
                cursor = cursor.saturating_sub(count);
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
            if context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::D)) {
                if let Some((bookmark, kind)) = entries.get(cursor) {
                    if matches!(kind, Some(places::Kind::Trash)) {
                        self.confirm_empty_trash = true;
                    } else {
                        self.remove_bookmark(bookmark, *kind);
                        entries.remove(cursor);
                        cursor = cursor.min(entries.len().saturating_sub(1));
                    }
                }
            }
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
        self.sidebar_cursor = Some(cursor);
        let mut frame = egui::Frame::side_top_panel(&context.style());
        frame.inner_margin.left = 0;
        egui::SidePanel::left(egui::Id::new(("bookmarks", context.viewport_id())))
            .frame(frame)
            .default_width(180.0)
            .width_range(140.0..=280.0)
            .show(context, |ui| {
                let drop = ui.interact(
                    ui.max_rect(),
                    ui.id().with("bookmark-drop"),
                    egui::Sense::hover(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size = Vec2::splat(24.0);
                    ui.spacing_mut().button_padding = Vec2::new(6.0, 4.0);
                    ui.add_space(12.0);
                    ui.label(RichText::new("BOOKMARKS").size(11.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("…", |ui| {
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
                        })
                        .response
                        .on_hover_text("Bookmark options");
                        if ui
                            .add(egui::Button::new("+").min_size(Vec2::splat(24.0)))
                            .on_hover_text("Add bookmark")
                            .clicked()
                        {
                            self.bookmark();
                        }
                    });
                });
                ui.add_space(8.0);
                with_scrollbar_color(ui, |ui, content_style| {
                    self.entry_scroll_area(ui, "bookmark-list")
                        .auto_shrink([false, false])
                        .drag_to_scroll(false)
                        .show(ui, |ui| {
                            ui.set_style(content_style);
                            let divider_x = ui.max_rect().left() + self.number_width();
                            let viewport = ui.clip_rect();
                            for (index, (bookmark, kind)) in entries.iter().enumerate() {
                                if index == default_count {
                                    ui.separator();
                                }
                                self.bookmark_row(ui, bookmark, *kind, index, context);
                            }
                            ui.painter().line_segment(
                                [
                                    egui::pos2(divider_x, viewport.top()),
                                    egui::pos2(divider_x, viewport.bottom()),
                                ],
                                ui.visuals().widgets.noninteractive.bg_stroke,
                            );
                        });
                });
                if drop
                    .dnd_hover_payload::<Vec<PathBuf>>()
                    .is_some_and(|paths| paths.iter().any(|path| path.is_dir()))
                {
                    ui.painter().rect_stroke(
                        ui.max_rect().shrink(2.0),
                        4.0,
                        Stroke::new(2.0, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(paths) = drop.dnd_release_payload::<Vec<PathBuf>>() {
                    for path in paths.iter().filter(|path| path.is_dir()) {
                        self.add_bookmark(path.clone());
                    }
                }
            });
    }

    fn message_banner(&mut self, ui: &mut egui::Ui) {
        let Some(message) = self.message.clone() else {
            return;
        };
        let (background, border, accent) = if ui.visuals().dark_mode {
            (
                Color32::from_rgb(46, 42, 33),
                Color32::from_rgb(91, 76, 47),
                Color32::from_rgb(222, 183, 110),
            )
        } else {
            (
                Color32::from_rgb(250, 246, 235),
                Color32::from_rgb(226, 213, 181),
                Color32::from_rgb(145, 105, 35),
            )
        };
        egui::Frame::new()
            .fill(background)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .outer_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let (icon, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                    ui.painter()
                        .circle_stroke(icon.center(), 7.5, Stroke::new(1.2, accent));
                    ui.painter().text(
                        icon.center(),
                        egui::Align2::CENTER_CENTER,
                        "!",
                        egui::FontId::proportional(13.0),
                        accent,
                    );
                    ui.add_sized(
                        [(ui.available_width() - 32.0).max(0.0), 0.0],
                        egui::Label::new(RichText::new(message).size(12.0)).wrap(),
                    );
                    if ui
                        .add_sized([24.0, 24.0], egui::Button::new("×").frame(false))
                        .clicked()
                    {
                        self.message = None;
                    }
                });
            });
    }

    fn statusbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::side_top_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(context, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 24.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(format!("{} items", self.entries.len()))
                                .size(12.0)
                                .color(MUTED),
                        );
                        ui.separator();
                        if self.media_scan.is_some() {
                            ui.spinner();
                            ui.label(RichText::new("Scanning media…").size(12.0).color(MUTED));
                            ui.separator();
                        }
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
                                RichText::new(format!("{} selected", self.selection.len()))
                                    .size(12.0),
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
                                ui.label(
                                    RichText::new("Calculating size…").size(12.0).color(MUTED),
                                );
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
                            let response = ui.add(
                                egui::Slider::new(self.settings.zoom_mut(), 0.65..=3.6)
                                    .show_value(false),
                            );
                            if response.changed() {
                                self.save();
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{}%",
                                    (self.settings.zoom() * 100.0).round() as u32
                                ))
                                .size(12.0)
                                .color(MUTED),
                            );
                            ui.separator();
                            if let Some(disk) = &self.disk {
                                ui.label(
                                    RichText::new(format!(
                                        "{} free",
                                        files::size_label(disk.available)
                                    ))
                                    .size(12.0)
                                    .color(ACCENT),
                                );
                            }
                            if let Some(job) = &self.operation_job {
                                let fraction = job.completed as f32 / job.total.max(1) as f32;
                                ui.add(
                                    egui::ProgressBar::new(fraction)
                                        .desired_width(ui.available_width().min(240.0))
                                        .desired_height(16.0)
                                        .text(
                                            RichText::new(format!(
                                                "{} · {}/{}",
                                                job.label, job.completed, job.total
                                            ))
                                            .size(12.0),
                                        ),
                                )
                                .on_hover_text(job.path.to_string_lossy());
                                ui.spinner();
                            } else if let Some((completed, total)) =
                                self.thumbnails.progress(context)
                            {
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
                            if self.thumbnails.warning().is_some() {
                                ui.label(
                                    RichText::new("Disk cache unavailable; using memory cache")
                                        .size(11.0)
                                        .color(MUTED),
                                );
                            }
                        });
                    },
                );
                if self.conversion_job.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Converting media…");
                    });
                }
                if self.compression_job.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Compressing to ZIP…");
                    });
                }
            });
    }

    fn entry_response(&mut self, response: egui::Response, entry: &Entry, context: &egui::Context) {
        if self.new_folder.as_ref().is_some_and(|dialog| {
            dialog.editing_path() == Some(&entry.path) || dialog.sources.contains(&entry.path)
        }) {
            return;
        }
        if response.drag_started() && !files::is_trash(&self.current) {
            let paths = if self.selection.contains(&entry.path) {
                self.visible_paths
                    .iter()
                    .filter(|path| self.selection.contains(*path))
                    .filter(|path| {
                        !path
                            .ancestors()
                            .skip(1)
                            .any(|parent| self.selection.contains(parent))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                self.select(Some(entry.path.clone()), context);
                vec![entry.path.clone()]
            };
            let paths = paths
                .into_iter()
                .flat_map(|path| {
                    self.entries
                        .iter()
                        .chain(self.tree_cache.values().flatten())
                        .find(|entry| entry.path == path)
                        .and_then(|entry| entry.sequence.as_ref())
                        .map_or_else(|| vec![path], |frames| frames.as_ref().clone())
                })
                .collect::<Vec<_>>();
            self.external_drag.start(paths.clone());
            response.dnd_set_drag_payload(paths);
        }
        if entry.directory {
            if response.dnd_hover_payload::<Vec<PathBuf>>().is_some() {
                context.layer_painter(response.layer_id).rect_stroke(
                    response.rect.shrink(2.0),
                    4.0,
                    Stroke::new(2.0, ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(paths) = response.dnd_release_payload::<Vec<PathBuf>>() {
                self.move_paths(&paths, &entry.path, context);
            }
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
        if response.secondary_clicked() {
            self.open_entry_menu(
                entry.clone(),
                response
                    .interact_pointer_pos()
                    .unwrap_or(response.rect.center()),
                true,
            );
        }
    }

    fn open_entry_menu(&mut self, entry: Entry, position: egui::Pos2, use_selection: bool) {
        self.visual_line = false;
        let mut root = egui::menu::MenuRootManager::default();
        *root = Some(egui::menu::MenuRoot::new(
            position,
            egui::Id::new("entry-menu"),
        ));
        self.entry_menu = Some(EntryMenu {
            root,
            entry,
            use_selection,
            cursor: 0,
            items: 0,
            activate: false,
            count: 0,
            pending_g: false,
        });
        self.motion_count = 0;
        self.pending_g = false;
        self.pending_z = false;
        self.pending_y = false;
        self.pending_d = false;
        self.pending_window_command = false;
    }

    fn entry_menu_shortcuts(&mut self, context: &egui::Context) {
        let Some(menu) = &mut self.entry_menu else {
            return;
        };
        let events = context.input_mut(|input| std::mem::take(&mut input.events));
        let mut close = false;
        for event in events {
            match event {
                egui::Event::Text(text) if text.bytes().all(|byte| byte.is_ascii_digit()) => {
                    for digit in text.bytes() {
                        menu.count = menu
                            .count
                            .saturating_mul(10)
                            .saturating_add((digit - b'0') as usize);
                    }
                }
                egui::Event::Text(text) if text == "_" => menu.cursor = 0,
                egui::Event::Text(text) if text == "$" => {
                    menu.cursor = menu.items.saturating_sub(1);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let last = menu.items.saturating_sub(1);
                    let count = menu.count.max(1).min(menu.items.max(1));
                    if modifiers.matches_exact(egui::Modifiers::NONE)
                        || modifiers.matches_exact(egui::Modifiers::SHIFT)
                            && matches!(
                                key,
                                Key::H
                                    | Key::J
                                    | Key::K
                                    | Key::L
                                    | Key::O
                                    | Key::ArrowUp
                                    | Key::ArrowDown
                                    | Key::ArrowLeft
                                    | Key::ArrowRight
                            )
                    {
                        match key {
                            Key::Escape => close = true,
                            Key::O | Key::Enter | Key::Space => menu.activate = true,
                            Key::G if menu.pending_g => {
                                menu.cursor = menu.count.saturating_sub(1).min(last);
                                menu.pending_g = false;
                                menu.count = 0;
                            }
                            Key::G => menu.pending_g = true,
                            Key::Home | Key::Minus => menu.cursor = 0,
                            Key::End => menu.cursor = last,
                            Key::J
                            | Key::L
                            | Key::ArrowDown
                            | Key::ArrowRight
                            | Key::W
                            | Key::E
                            | Key::Tab => {
                                menu.cursor = menu.cursor.saturating_add(count).min(last);
                                menu.count = 0;
                                menu.pending_g = false;
                            }
                            Key::K | Key::H | Key::ArrowUp | Key::ArrowLeft | Key::B => {
                                menu.cursor = menu.cursor.saturating_sub(count);
                                menu.count = 0;
                                menu.pending_g = false;
                            }
                            _ => {}
                        }
                    } else if modifiers.matches_exact(egui::Modifiers::SHIFT) {
                        match key {
                            Key::G => {
                                menu.cursor = if menu.count == 0 {
                                    last
                                } else {
                                    menu.count.saturating_sub(1).min(last)
                                };
                                menu.count = 0;
                                menu.pending_g = false;
                            }
                            Key::Tab => menu.cursor = menu.cursor.saturating_sub(1),
                            _ => {}
                        }
                    } else if modifiers.matches_exact(egui::Modifiers::CTRL) {
                        match key {
                            Key::N | Key::P => {
                                menu.cursor = if key == Key::N {
                                    menu.cursor.saturating_add(count).min(last)
                                } else {
                                    menu.cursor.saturating_sub(count)
                                };
                                menu.count = 0;
                                menu.pending_g = false;
                            }
                            Key::D => {
                                menu.cursor =
                                    menu.cursor.saturating_add(menu.items.div_ceil(2)).min(last)
                            }
                            Key::U => {
                                menu.cursor = menu.cursor.saturating_sub(menu.items.div_ceil(2))
                            }
                            _ => {}
                        }
                    }
                }
                egui::Event::Key { .. } | egui::Event::Text(_) => {}
                event => context.input_mut(|input| input.events.push(event)),
            }
        }
        if close {
            self.entry_menu = None;
        }
    }

    fn show_entry_menu(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let Some(menu) = &mut self.entry_menu else {
            return;
        };
        let response = ui.interact(
            egui::Rect::NOTHING,
            egui::Id::new("entry-menu"),
            egui::Sense::hover(),
        );
        let mut root = std::mem::take(&mut menu.root);
        egui::menu::MenuRoot::context_click_interaction(&response, &mut root);
        let entry = menu.entry.clone();
        menu.cursor = menu.cursor.min(menu.items.saturating_sub(1));
        menu.items = 0;
        root.show(&response, |ui| {
            self.entry_menu_contents(ui, &entry, context)
        });
        if root.is_none() {
            self.entry_menu = None;
        } else if let Some(menu) = &mut self.entry_menu {
            menu.root = root;
            menu.activate = false;
        }
    }

    fn entry_menu_button(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        icon: &str,
        enabled: bool,
        tooltip: Option<&str>,
    ) -> bool {
        let Some(menu) = &mut self.entry_menu else {
            return false;
        };
        let selected = enabled && menu.items == menu.cursor;
        let response = ui
            .scope(|ui| {
                if selected {
                    ui.visuals_mut().widgets.inactive = ui.visuals().widgets.hovered;
                }
                self.icons.button(ui, label, icon, enabled)
            })
            .inner;
        let response = if let Some(tooltip) = tooltip {
            response.on_hover_text(tooltip)
        } else {
            response
        };
        if enabled {
            if response.hovered() && ui.input(|input| input.pointer.delta() != Vec2::ZERO) {
                menu.cursor = menu.items;
            }
            menu.items += 1;
        }
        let activated = response.clicked() || (selected && menu.activate);
        if activated {
            self.visual_line = false;
        }
        activated
    }

    fn entry_menu_contents(&mut self, ui: &mut egui::Ui, entry: &Entry, context: &egui::Context) {
        let multimedia = entry.sequence.is_some()
            || files::image_file(&entry.path)
            || files::video_file(&entry.path);
        if self.entry_menu_button(
            ui,
            if multimedia {
                "Open with Player"
            } else {
                "Open"
            },
            if multimedia {
                "media-playback-start"
            } else {
                "document-open"
            },
            true,
            None,
        ) {
            if multimedia {
                self.choose_media_player(&entry.path, "player");
            } else {
                self.open(entry.path.clone(), context);
            }
            ui.close_menu();
        }
        if multimedia {
            for (application, label) in [("vlc", "Open with VLC"), ("mpv", "Open with MPV")] {
                if self.entry_menu_button(ui, label, application, true, None) {
                    self.choose_media_player(&entry.path, application);
                    ui.close_menu();
                }
            }
        }
        if !entry.directory && files::archive_file(&entry.path) && !files::is_trash(&self.current) {
            ui.separator();
            if self.entry_menu_button(
                ui,
                "Extract here",
                "package-x-generic",
                self.operation_job.is_none(),
                None,
            ) {
                self.start_operation(FileOperation::Extract(entry.path.clone()), context);
                ui.close_menu();
            }
            ui.separator();
        }
        if multimedia && !files::is_trash(&self.current) {
            ui.separator();
            for format in [
                conversion::Format::Png,
                conversion::Format::Mp4,
                conversion::Format::Mov,
            ] {
                if self.entry_menu_button(
                    ui,
                    format.label(),
                    "media-record",
                    self.conversion_job.is_none(),
                    (format == conversion::Format::Mov).then_some("Apple ProRes 422 HQ"),
                ) {
                    self.convert(entry.clone(), format, context);
                    ui.close_menu();
                }
            }
            ui.separator();
        }
        if entry.directory {
            if self.entry_menu_button(ui, "Open Konsole here", "utilities-terminal", true, None) {
                self.open_terminal(&entry.path);
                ui.close_menu();
            }
            if self.entry_menu_button(ui, "Add bookmark", "bookmark-new", true, None) {
                self.add_bookmark(entry.path.clone());
                ui.close_menu();
            }
        }
        if entry.sequence.is_none()
            && (entry.directory || files::image_file(&entry.path))
            && self.entry_menu_button(
                ui,
                "Compress to ZIP",
                "package-x-generic",
                self.compression_job.is_none(),
                None,
            )
        {
            self.compress(entry.path.clone(), context);
            ui.close_menu();
        }
        if self.entry_menu_button(ui, "Copy path", "edit-copy", true, None) {
            context.copy_text(entry.path.to_string_lossy().into_owned());
            ui.close_menu();
        }
        ui.separator();
        if !files::is_trash(&self.current)
            && self.entry_menu_button(ui, "Move to Trash · Delete", "user-trash", true, None)
        {
            if self
                .entry_menu
                .as_ref()
                .is_some_and(|menu| menu.use_selection)
            {
                self.trash_selection(context);
            } else {
                self.start_operation(FileOperation::Trash(vec![entry.path.clone()]), context);
            }
            ui.close_menu();
        }
        if self.entry_menu_button(ui, "Properties", "document-properties", true, None) {
            self.show_properties(&entry.path);
            ui.close_menu();
        }
    }

    fn paint_entry_icon(&mut self, ui: &egui::Ui, rect: egui::Rect, entry: &Entry) {
        if !ui.is_rect_visible(rect) {
            return;
        }
        self.prepared_view.push((entry.path.clone(), rect));
        if entry.directory && entry.has_contents.is_none() && entry.item_count.is_none() {
            self.directory_ready = false;
        }
        let texture = if entry.directory {
            None
        } else {
            self.icons.get(&entry.path)
        };
        if let Some(texture) = texture {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if entry.symlink {
                ui.painter().text(
                    rect.right_bottom(),
                    egui::Align2::RIGHT_BOTTOM,
                    "@",
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        } else {
            if !entry.directory {
                self.directory_ready &= self.icons.ready(&entry.path);
            }
            paint_icon(ui, rect, entry);
        }
        paint_deletion_lock(ui, rect, entry.deletion_locked);
    }

    fn paint_thumbnail(&mut self, ui: &egui::Ui, rect: egui::Rect, entry: &Entry) -> bool {
        let video = entry.animated();
        if entry.directory
            || (!files::image_file(&entry.path) && !video)
            || !ui.is_rect_visible(rect)
        {
            return false;
        }
        self.prepared_view.push((entry.path.clone(), rect));
        let image_bounds = rect;
        let Some(texture) = self.thumbnails.get_entry(entry) else {
            if self.wait_for_thumbnails {
                self.directory_ready &= self.thumbnails.entry_ready(entry);
            }
            paint_thumbnail_placeholder(ui, image_bounds);
            paint_deletion_lock(ui, image_bounds, entry.deletion_locked);
            return true;
        };
        let dimensions = texture.size_vec2();
        let scale = (image_bounds.width() / dimensions.x).min(image_bounds.height() / dimensions.y);
        let image_rect = egui::Rect::from_center_size(image_bounds.center(), dimensions * scale);
        let uv_size = image_rect.size() / (dimensions * scale);
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_center_size(egui::pos2(0.5, 0.5), uv_size),
            if video {
                Color32::WHITE
            } else {
                texture_tint(ui, texture)
            },
        );
        paint_deletion_lock(ui, image_rect, entry.deletion_locked);
        true
    }

    fn search_results(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let mut open = None;
        let mut select = None;
        if let Some(search) = &mut self.search {
            egui::Frame::new().inner_margin(16.0).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search results").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Esc to close").small().color(MUTED));
                    });
                });
                let (position, galley, _) =
                    egui::Label::new(RichText::new(self.current.to_string_lossy()).color(MUTED))
                        .truncate()
                        .layout_in_ui(ui);
                ui.painter().galley(position, galley, MUTED);
                ui.add_space(6.0);
                let status = if let Some(update) = &search.update {
                    let mut status = if update.error.is_some() {
                        "Search unavailable".to_owned()
                    } else {
                        format!("{} matches", update.matched)
                    };
                    status.push_str(&format!(
                        " · {} items {}",
                        update.indexed,
                        if update.scanning {
                            "scanned…"
                        } else {
                            "scanned"
                        }
                    ));
                    if update.matched > update.hits.len() {
                        status.push_str(&format!(" · Best {} shown", update.hits.len()));
                    }
                    if update.skipped > 0 {
                        status.push_str(&format!(" · {} unreadable items skipped", update.skipped));
                    }
                    status
                } else {
                    "Searching this folder and subfolders…".to_owned()
                };
                let (position, galley, _) = egui::Label::new(RichText::new(status).color(MUTED))
                    .truncate()
                    .layout_in_ui(ui);
                ui.painter().galley(position, galley, MUTED);
                ui.add_space(6.0);
                ui.separator();
                if let Some(update) = &search.update {
                    if update.error.is_some() || (update.hits.is_empty() && !update.scanning) {
                        ui.add_space(48.0);
                        ui.vertical_centered(|ui| {
                            if let Some(error) = &update.error {
                                ui.label(
                                    RichText::new("Search could not finish").strong().size(18.0),
                                );
                                ui.add_space(8.0);
                                ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
                                ui.label(
                                    RichText::new(
                                        "Check the search engine, then press F5 to retry.",
                                    )
                                    .color(MUTED),
                                );
                            } else {
                                ui.label(RichText::new("No matching files").strong().size(18.0));
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new("Try fewer characters or a different name.")
                                        .color(MUTED),
                                );
                                ui.label(
                                    RichText::new(
                                        "Hidden files are included only when Hidden is enabled.",
                                    )
                                    .small()
                                    .color(MUTED),
                                );
                            }
                        });
                    }
                    let row_height = 36.0;
                    let name_width =
                        ((ui.available_width() - ui.spacing().scroll.allocated_width()) * 0.42)
                            .max(100.0);
                    if !update.hits.is_empty() {
                        let (header, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 28.0),
                            egui::Sense::hover(),
                        );
                        for (offset, title) in [(40.0, "Name"), (name_width, "Location")] {
                            ui.painter().text(
                                egui::pos2(header.left() + offset, header.center().y),
                                egui::Align2::LEFT_CENTER,
                                title,
                                egui::FontId::proportional(12.0),
                                MUTED,
                            );
                        }
                    }
                    with_scrollbar_color(ui, |ui, content_style| {
                        egui::ScrollArea::vertical()
                            .id_salt("search-results")
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, update.hits.len(), |ui, range| {
                                ui.set_style(content_style);
                                if search.reveal_cursor && !update.hits.is_empty() {
                                    let height = row_height + ui.spacing().item_spacing.y;
                                    let top = ui.max_rect().top()
                                        + (search.cursor as f32 - range.start as f32) * height;
                                    ui.scroll_to_rect(
                                        egui::Rect::from_min_size(
                                            egui::pos2(ui.max_rect().left(), top),
                                            Vec2::new(ui.available_width(), row_height),
                                        ),
                                        None,
                                    );
                                    search.reveal_cursor = false;
                                }
                                for index in range {
                                    let hit = &update.hits[index];
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), row_height),
                                        egui::Sense::click(),
                                    );
                                    if index % 2 == 1 {
                                        ui.painter().rect_filled(
                                            rect,
                                            0.0,
                                            ui.visuals().faint_bg_color,
                                        );
                                    }
                                    paint_selection(
                                        ui,
                                        rect,
                                        !self.sidebar_focused && search.cursor == index,
                                        response.hovered(),
                                    );
                                    let icon_rect = egui::Rect::from_center_size(
                                        egui::pos2(rect.left() + 20.0, rect.center().y),
                                        Vec2::splat(22.0),
                                    );
                                    self.prepared_view.push((hit.path.clone(), icon_rect));
                                    if hit.directory {
                                        paint_folder(ui, icon_rect);
                                    } else if let Some(texture) = self.icons.get(&hit.path) {
                                        ui.painter().image(
                                            texture.id(),
                                            icon_rect,
                                            egui::Rect::from_min_max(
                                                egui::Pos2::ZERO,
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        self.directory_ready &= self.icons.ready(&hit.path);
                                        ui.painter().rect_stroke(
                                            icon_rect.shrink2(Vec2::new(4.0, 1.0)),
                                            2.0,
                                            Stroke::new(1.5, MUTED),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    paint_deletion_lock(ui, icon_rect, hit.deletion_locked);
                                    let name =
                                        hit.path.file_name().unwrap_or_default().to_string_lossy();
                                    let location =
                                        hit.path.parent().unwrap_or(&hit.path).to_string_lossy();
                                    for (text, left, right, color) in [
                                        (name, 40.0, name_width - 16.0, ui.visuals().text_color()),
                                        (location, name_width, rect.width() - 12.0, MUTED),
                                    ] {
                                        let text_rect = egui::Rect::from_min_max(
                                            egui::pos2(rect.left() + left, rect.top()),
                                            egui::pos2(rect.left() + right, rect.bottom()),
                                        );
                                        let mut cell = ui.new_child(
                                            egui::UiBuilder::new().max_rect(text_rect).layout(
                                                egui::Layout::left_to_right(egui::Align::Center),
                                            ),
                                        );
                                        cell.set_clip_rect(ui.clip_rect().intersect(text_rect));
                                        let (position, galley, _) =
                                            egui::Label::new(RichText::new(text).color(color))
                                                .truncate()
                                                .layout_in_ui(&mut cell);
                                        cell.painter().galley(position, galley, color);
                                    }
                                    if response.clicked() {
                                        search.cursor = index;
                                        select = Some(hit.path.clone());
                                    }
                                    if response.double_clicked() {
                                        open = Some(hit.path.clone());
                                    }
                                }
                            });
                    });
                }
            });
        }
        if let Some(path) = select {
            self.select(Some(path), context);
        }
        if let Some(path) = open {
            self.open(path, context);
        }
    }

    fn display_entries(&self) -> Arc<Vec<Entry>> {
        self.new_folder
            .as_ref()
            .filter(|dialog| dialog.parent == self.current)
            .and_then(|dialog| dialog.preview.as_ref())
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.entries))
    }

    fn marquee_selection(
        &mut self,
        ui: &egui::Ui,
        background: &egui::Response,
        origin: egui::Pos2,
        items: impl Iterator<Item = (PathBuf, egui::Rect)>,
    ) {
        self.directory_background(background, ui.ctx());
        if background.drag_started_by(egui::PointerButton::Primary) {
            let Some(pointer) = ui.input(|input| input.pointer.press_origin()) else {
                return;
            };
            let additive = ui.input(|input| input.modifiers.shift || input.modifiers.command);
            self.marquee_selection = Some(MarqueeSelection {
                origin: pointer - origin.to_vec2(),
                initial: if additive {
                    self.selection.clone()
                } else {
                    HashSet::new()
                },
            });
            self.sidebar_focused = false;
        }
        let Some(marquee) = &self.marquee_selection else {
            return;
        };
        if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
            let rect = egui::Rect::from_two_pos(marquee.origin + origin.to_vec2(), pointer);
            let mut selection = marquee.initial.clone();
            selection.extend(
                items
                    .filter(|(_, item)| rect.intersects(*item))
                    .map(|(path, _)| path),
            );
            if selection != self.selection {
                let first = self
                    .visible_paths
                    .iter()
                    .find(|path| selection.contains(*path))
                    .cloned();
                self.select(first, ui.ctx());
                self.selection = selection;
                if self.selection.len() > 1 {
                    self.size_job = None;
                    self.selection_size = None;
                }
                ui.ctx().request_repaint();
            }
            ui.painter().rect_filled(
                rect,
                0.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.3),
            );
            ui.painter().rect_stroke(
                rect,
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        if !ui.input(|input| input.pointer.primary_down()) {
            self.marquee_selection = None;
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let entries = self.display_entries();
        let width = 116.0 * self.settings.grid_zoom;
        let height = 126.0 * self.settings.grid_zoom;
        let gutter = self.number_width();
        let available = ui.available_width() - ui.spacing().scroll.allocated_width() - gutter;
        let columns = ((available - 16.0 + 8.0) / (width + 8.0)).floor().max(1.0) as usize;
        let rows = entries.len().div_ceil(columns);
        let items: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                (
                    entry,
                    egui::pos2(
                        (index % columns) as f32 * (width + ui.spacing().item_spacing.x),
                        (index / columns) as f32 * (height + ui.spacing().item_spacing.y),
                    ),
                    height,
                )
            })
            .collect();
        ui.scope(|ui| {
            let content_height =
                rows as f32 * (height + ui.spacing().item_spacing.y) - ui.spacing().item_spacing.y;
            self.navigate_items(&items, ui, "grid", content_height.max(0.0), context);
            let current_row = entries
                .iter()
                .position(|entry| self.selected.as_ref() == Some(&entry.path))
                .unwrap_or(0)
                / columns;
            let mut scroll = self.entry_scroll_area(ui, "grid");
            if self.scroll_to_selection {
                if let Some(index) = entries
                    .iter()
                    .position(|entry| self.selected.as_ref() == Some(&entry.path))
                {
                    let top = (index / columns) as f32 * (height + ui.spacing().item_spacing.y);
                    let viewport = ui.available_height();
                    let offset = egui::scroll_area::State::load(
                        context,
                        ui.make_persistent_id(egui::Id::new("grid")),
                    )
                    .map_or(0.0, |state| state.offset.y);
                    let offset = if self.center_selection {
                        top + (height - viewport) / 2.0
                    } else {
                        offset.min(top).max(top + height - viewport)
                    };
                    let content_height = rows as f32 * (height + ui.spacing().item_spacing.y)
                        - ui.spacing().item_spacing.y;
                    let max_offset = (content_height - viewport).max(0.0);
                    scroll = scroll.vertical_scroll_offset(offset.clamp(0.0, max_offset));
                }
                self.scroll_to_selection = false;
                self.center_selection = false;
            }
            with_scrollbar_color(ui, |ui, content_style| {
                scroll
                    .auto_shrink([false, false])
                    .id_salt("grid")
                    .drag_to_scroll(false)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show_rows(ui, height, rows, |ui, range| {
                        ui.set_style(content_style);
                        let background = ui.interact(
                            ui.clip_rect(),
                            ui.id().with("marquee"),
                            egui::Sense::click_and_drag(),
                        );
                        let mut origin = None;
                        let start = (range.start * columns).min(entries.len());
                        let end = (range.end * columns).min(entries.len());
                        for (index, row) in entries[start..end].chunks(columns).enumerate() {
                            ui.horizontal(|ui| {
                                if gutter > 0.0 {
                                    let (rect, _) = ui.allocate_exact_size(
                                        Vec2::new(gutter, height),
                                        egui::Sense::hover(),
                                    );
                                    self.paint_relative_number(
                                        ui,
                                        rect,
                                        range.start + index,
                                        current_row,
                                    );
                                }
                                ui.add_space(8.0);
                                for entry in row {
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(width, height),
                                        egui::Sense::click_and_drag(),
                                    );
                                    origin.get_or_insert(
                                        rect.min
                                            - Vec2::new(
                                                0.0,
                                                range.start as f32
                                                    * (height + ui.spacing().item_spacing.y),
                                            ),
                                    );
                                    if !self.sidebar_focused && self.selection.contains(&entry.path)
                                    {
                                        ui.painter().rect_filled(
                                            rect.shrink(2.0),
                                            8.0,
                                            ui.visuals().selection.bg_fill,
                                        );
                                    }
                                    let icon_rect = egui::Rect::from_center_size(
                                        rect.center_top() + Vec2::new(0.0, height * 0.34),
                                        Vec2::splat(64.0 * self.settings.grid_zoom),
                                    );
                                    if !self.paint_thumbnail(ui, icon_rect, entry) {
                                        self.paint_entry_icon(
                                            ui,
                                            icon_rect.shrink(8.0 * self.settings.grid_zoom),
                                            entry,
                                        );
                                    }
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            rect.shrink(2.0),
                                            8.0,
                                            ui.visuals().panel_fill.gamma_multiply(0.18),
                                        );
                                    }
                                    let text_color = if response.hovered() {
                                        ACCENT
                                    } else {
                                        ui.visuals().text_color()
                                    };
                                    let name_rect = egui::Rect::from_min_size(
                                        egui::pos2(rect.left() + 7.0, rect.top() + height * 0.66),
                                        Vec2::new(width - 14.0, 24.0),
                                    );
                                    if !self.rename_field(ui, name_rect, entry, context) {
                                        let mut job = egui::text::LayoutJob::simple(
                                            entry.name.clone(),
                                            egui::FontId::proportional(12.0),
                                            text_color,
                                            width - 14.0,
                                        );
                                        job.wrap.max_rows = 2;
                                        job.halign = egui::Align::Center;
                                        let galley = ui.painter().layout_job(job);
                                        ui.painter().galley(
                                            egui::pos2(rect.center().x, rect.top() + height * 0.66),
                                            galley,
                                            text_color,
                                        );
                                    }
                                    self.entry_response(response, entry, context);
                                }
                            });
                        }
                        if let Some(origin) = origin {
                            self.marquee_selection(
                                ui,
                                &background,
                                origin,
                                entries.iter().enumerate().map(|(index, entry)| {
                                    let position = origin
                                        + Vec2::new(
                                            (index % columns) as f32
                                                * (width + ui.spacing().item_spacing.x),
                                            (index / columns) as f32
                                                * (height + ui.spacing().item_spacing.y),
                                        );
                                    (
                                        entry.path.clone(),
                                        egui::Rect::from_min_size(
                                            position,
                                            Vec2::new(width, height),
                                        ),
                                    )
                                }),
                            );
                        }
                    });
            });
        });
    }

    fn prepare_media_layout(&mut self, width: f32, target: f32, context: &egui::Context) {
        let source = self.display_entries();
        let source_changed = self.media_layout.source.as_ptr() != Arc::as_ptr(&source);
        let resized = self.media_layout.width != width || self.media_layout.target != target;
        if !source_changed
            && !resized
            && !self.media_layout.items.is_empty()
            && (self.media_scroll_target.is_some()
                || context.input(|input| input.time) < self.media_scrolling_until)
        {
            context.request_repaint_after(std::time::Duration::from_millis(150));
            return;
        }
        if !source_changed
            && !resized
            && self.media_layout.thumbnail_revision == self.thumbnails.revision()
            && (self.media_layout.complete || self.thumbnails.is_loading())
        {
            if !self.media_layout.complete {
                context.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        if source_changed {
            let indices: HashMap<_, _> = source
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    !entry.directory
                        && (entry.sequence.is_some()
                            || files::image_file(&entry.path)
                            || files::video_file(&entry.path))
                })
                .map(|(index, entry)| (entry.path.clone(), index))
                .collect();
            if self
                .media_layout
                .paths
                .iter()
                .any(|path| !indices.contains_key(path))
            {
                self.media_layout = MediaLayout::default();
            }
            self.media_layout.indices = indices;
            self.media_layout.source = Arc::downgrade(&source);
        }
        if !source_changed
            && !resized
            && self
                .media_layout
                .updated
                .is_some_and(|updated| updated.elapsed() < std::time::Duration::from_millis(100))
        {
            context.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        self.media_layout.updated = Some(std::time::Instant::now());
        self.media_layout.thumbnail_revision = self.thumbnails.revision();
        let previous_count = self.media_layout.paths.len();
        self.media_layout
            .paths
            .retain(|path| self.media_layout.indices.contains_key(path));
        let mut removed = self.media_layout.paths.len() != previous_count;
        if removed {
            self.media_layout
                .aspects
                .retain(|path, _| self.media_layout.indices.contains_key(path));
            self.media_layout.rows.clear();
        }
        let resized =
            resized || self.media_layout.width != width || self.media_layout.target != target;
        if resized {
            self.media_layout.rows.clear();
            self.media_layout.width = width;
            self.media_layout.target = target;
        }
        let mut resolved = 0;
        for entry in source
            .iter()
            .filter(|entry| self.media_layout.indices.contains_key(&entry.path))
        {
            resolved += 1;
            let aspect = if entry.directory {
                1.0
            } else {
                self.thumbnails
                    .entry_aspect_ratio(entry)
                    .unwrap_or_else(|| {
                        self.media_layout
                            .aspects
                            .get(&entry.path)
                            .copied()
                            .unwrap_or(16.0 / 9.0)
                    })
            };
            match self.media_layout.aspects.insert(entry.path.clone(), aspect) {
                Some(previous) if previous != aspect => {
                    self.media_layout.rows.clear();
                    removed = true;
                }
                Some(_) => {}
                None => self.media_layout.paths.push(entry.path.clone()),
            }
        }
        if source_changed {
            let previous = self.media_layout.paths.clone();
            self.media_layout
                .paths
                .sort_by_key(|path| self.media_layout.indices[path]);
            if previous != self.media_layout.paths {
                self.media_layout.rows.clear();
                removed = true;
            }
        }
        let complete = self.media_scan.is_none() && resolved == self.media_layout.indices.len();
        let was_complete = self.media_layout.complete;
        self.media_layout.complete = complete;
        if !complete {
            context.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if !source_changed && !resized && !removed && was_complete == complete {
            return;
        }
        let aspects: Vec<_> = self
            .media_layout
            .paths
            .iter()
            .map(|path| self.media_layout.aspects[path])
            .collect();
        if self
            .media_layout
            .rows
            .last()
            .is_some_and(|(range, height)| {
                range.end < aspects.len()
                    && aspects[range.clone()].iter().sum::<f32>() * height < width - 0.5
            })
        {
            self.media_layout.rows.pop();
        }
        let start = self
            .media_layout
            .rows
            .last()
            .map_or(0, |(range, _)| range.end);
        let mut new_rows = layout::justified_rows(&aspects[start..], width, target);
        if !complete
            && !self.media_layout.rows.is_empty()
            && self
                .new_folder
                .as_ref()
                .is_none_or(|dialog| dialog.preview.is_none())
            && new_rows.last().is_some_and(|(range, _)| {
                aspects[start + range.start..start + range.end]
                    .iter()
                    .sum::<f32>()
                    * target
                    < width
            })
        {
            new_rows.pop();
        }
        self.media_layout.rows.extend(
            new_rows
                .into_iter()
                .map(|(range, height)| (range.start + start..range.end + start, height)),
        );
        let count = self
            .media_layout
            .rows
            .last()
            .map_or(0, |(range, _)| range.end);
        if source_changed || resized || removed || self.media_layout.items.len() != count {
            let mut items = Vec::with_capacity(count);
            let mut y = 0.0;
            for (range, height) in &self.media_layout.rows {
                let mut x = 0.0;
                for index in range.clone() {
                    let path = &self.media_layout.paths[index];
                    let entry = source[self.media_layout.indices[path]].clone();
                    let item_width = aspects[index] * height;
                    items.push((entry, egui::pos2(x + item_width / 2.0, y), *height));
                    x += item_width;
                }
                y += height;
            }
            self.media_layout.items = Arc::new(items);
        }
    }

    fn media(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let gutter = self.number_width();
        let width =
            (ui.available_width() - ui.spacing().scroll.allocated_width() - gutter).max(1.0);
        let (now, dt, manual_scroll, navigating) = context.input(|input| {
            (
                input.time,
                input.stable_dt.min(0.05),
                input.raw_scroll_delta != Vec2::ZERO
                    || input.smooth_scroll_delta != Vec2::ZERO
                    || input.pointer.any_down(),
                [
                    Key::ArrowUp,
                    Key::ArrowDown,
                    Key::ArrowLeft,
                    Key::ArrowRight,
                    Key::H,
                    Key::J,
                    Key::K,
                    Key::L,
                ]
                .into_iter()
                .any(|key| input.key_down(key)),
            )
        });
        if manual_scroll {
            self.media_scroll_target = None;
        }
        if manual_scroll || navigating || self.media_scroll_target.is_some() {
            self.media_scrolling_until = now + 0.15;
        }
        let scroll_id = ui.make_persistent_id(egui::Id::new("media"));
        let mut offset =
            egui::scroll_area::State::load(context, scroll_id).map_or(0.0, |state| state.offset.y);
        let previous_items = Arc::clone(&self.media_layout.items);
        self.prepare_media_layout(width, 192.0 * self.settings.media_zoom, context);
        let items = Arc::clone(&self.media_layout.items);
        if !Arc::ptr_eq(&previous_items, &items) {
            let anchor = previous_items
                .partition_point(|(_, position, height)| position.y + height <= offset);
            if let Some((entry, previous, _)) = previous_items.get(anchor) {
                if let Some((_, position, _)) =
                    items.iter().find(|(item, _, _)| item.path == entry.path)
                {
                    let shift = position.y - previous.y;
                    offset += shift;
                    if let Some(target) = &mut self.media_scroll_target {
                        *target += shift;
                    }
                }
            }
        }
        if items.is_empty() {
            ui.centered_and_justified(|ui| {
                if self.media_layout.complete {
                    ui.label(RichText::new("No images or videos in this folder").color(MUTED));
                } else {
                    ui.spinner();
                }
            });
            return;
        }
        let rows = self.media_layout.rows.clone();
        let complete = self.media_layout.complete;
        let count = items.len();
        let total_height: f32 = rows.iter().map(|(_, height)| height).sum();
        let navigation: Vec<_> = items
            .iter()
            .map(|(entry, position, height)| (entry, *position, *height))
            .collect();
        self.navigate_items(&navigation, ui, "media", total_height, context);
        let current_index = items
            .iter()
            .position(|(entry, _, _)| self.selected.as_ref() == Some(&entry.path))
            .unwrap_or(0);
        let current_row = rows
            .iter()
            .position(|(range, _)| range.contains(&current_index))
            .unwrap_or(0);
        let viewport = ui.available_height();
        let max_offset = (total_height - viewport).max(0.0);
        offset = offset.clamp(0.0, max_offset);
        let page_delta = std::mem::take(&mut self.page_scroll_delta);
        if page_delta != 0.0 {
            self.media_scroll_target = Some((offset + page_delta).clamp(0.0, max_offset));
        }
        if self.scroll_to_selection {
            if let Some((_, position, height)) = items.get(current_index) {
                let target = if self.center_selection {
                    position.y + (height - viewport) / 2.0
                } else {
                    self.media_scroll_target
                        .unwrap_or(offset)
                        .min(position.y)
                        .max(position.y + height - viewport)
                };
                self.media_scroll_target = Some(target.clamp(0.0, max_offset));
            }
            self.scroll_to_selection = false;
            self.center_selection = false;
        }
        if let Some(target) = self.media_scroll_target {
            let target = target.clamp(0.0, max_offset);
            offset += (target - offset) * (1.0 - (-20.0 * dt).exp());
            if (target - offset).abs() < 0.5 {
                offset = target;
                self.media_scroll_target = None;
            } else {
                self.media_scroll_target = Some(target);
                context.request_repaint();
            }
        }
        with_scrollbar_color(ui, |ui, content_style| {
            self.entry_scroll_area(ui, "media")
                .vertical_scroll_offset(offset)
                .animated(false)
                .auto_shrink([false, false])
                .drag_to_scroll(false)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show_viewport(ui, |ui, _| {
                    ui.set_style(content_style);
                    let background = ui.interact(
                        ui.clip_rect(),
                        ui.id().with("media-background"),
                        egui::Sense::click(),
                    );
                    ui.set_min_size(Vec2::new(width + gutter, total_height));
                    let origin = ui.min_rect().min;
                    let mut y = origin.y;
                    for (row, (range, height)) in rows.into_iter().enumerate() {
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(origin.x, y),
                            Vec2::new(width + gutter, height),
                        );
                        if !ui.is_rect_visible(row_rect) {
                            y += height;
                            continue;
                        }
                        self.paint_relative_number(
                            ui,
                            egui::Rect::from_min_size(
                                egui::pos2(origin.x, y),
                                Vec2::new(gutter, height),
                            ),
                            row,
                            current_row,
                        );
                        let mut x = origin.x + gutter;
                        for index in range {
                            let (entry, position, _) = &items[index];
                            let center_x = origin.x + gutter + position.x;
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(x, y),
                                Vec2::new((center_x - x) * 2.0, height),
                            );
                            x = rect.right();
                            if !ui.is_rect_visible(rect) {
                                continue;
                            }
                            let response = ui.interact(
                                rect,
                                ui.id().with(&entry.path),
                                egui::Sense::click_and_drag(),
                            );
                            let highlighted =
                                !self.sidebar_focused && self.selection.contains(&entry.path);
                            ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
                            if !self.paint_thumbnail(ui, rect, entry) {
                                let edge = (64.0 * self.settings.media_zoom)
                                    .min(rect.width().min(rect.height()) * 0.5);
                                self.paint_entry_icon(
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
                            if response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    0.0,
                                    ui.visuals().panel_fill.gamma_multiply(0.18),
                                );
                            }
                            if highlighted {
                                ui.painter().rect_filled(
                                    rect,
                                    0.0,
                                    ui.visuals().selection.stroke.color.gamma_multiply(0.35),
                                );
                                let depth = (rect.width().min(rect.height()) * 0.25).min(28.0);
                                let mut shadow = egui::Mesh::default();
                                for ring in 0..=24_u8 {
                                    let progress = f32::from(ring) / 24.0;
                                    let fade = 1.0 - progress * progress * (3.0 - 2.0 * progress);
                                    let color =
                                        Color32::from_black_alpha((20.0 * fade).round() as u8);
                                    let inner = rect.shrink(depth * progress);
                                    for corner in [
                                        inner.left_top(),
                                        inner.right_top(),
                                        inner.right_bottom(),
                                        inner.left_bottom(),
                                    ] {
                                        shadow.colored_vertex(corner, color);
                                    }
                                    if ring > 0 {
                                        let outer = u32::from(ring - 1) * 4;
                                        let inner = outer + 4;
                                        for corner in 0..4 {
                                            let next = (corner + 1) % 4;
                                            shadow.add_triangle(
                                                outer + corner,
                                                outer + next,
                                                inner + corner,
                                            );
                                            shadow.add_triangle(
                                                outer + next,
                                                inner + next,
                                                inner + corner,
                                            );
                                        }
                                    }
                                }
                                ui.painter().add(egui::Shape::mesh(shadow));
                            }
                            paint_media_badge(ui, rect, entry);
                            let name_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    rect.left() + 4.0,
                                    (rect.bottom() - 26.0).max(rect.top()),
                                ),
                                egui::pos2(rect.right() - 4.0, rect.bottom() - 2.0),
                            );
                            self.rename_field(ui, name_rect, entry, context);
                            self.entry_response(response, entry, context);
                            if !complete && index + 1 == count {
                                let size = 24.0_f32.min(rect.width().min(rect.height()) * 0.5);
                                let center = rect.right_bottom() - Vec2::splat(size * 0.75);
                                ui.painter().circle_filled(
                                    center,
                                    size * 0.65,
                                    ui.visuals().panel_fill,
                                );
                                ui.put(
                                    egui::Rect::from_center_size(center, Vec2::splat(size)),
                                    egui::Spinner::new().size(size),
                                );
                            }
                        }
                        y += height;
                    }
                    self.directory_background(&background, context);
                });
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
                row.left() + self.number_width() + 36.0,
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
                self.settings
                    .entry_order
                    .retain(|directory, _| !directory.starts_with(&self.current));
                self.tree_layout = None;
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
        if let Some(order) = entries
            .first()
            .and_then(|entry| entry.path.parent())
            .and_then(|directory| self.settings.entry_order.get(directory))
        {
            order_entries(entries, order);
        }
        if let Some(dialog) = &self.new_folder {
            if let Some(path) = dialog.editing_path() {
                if path.parent() == entries.first().and_then(|entry| entry.path.parent()) {
                    if let Some(order) = dialog.creation_order(path) {
                        order_entries(entries, &order);
                    }
                }
            }
        }
    }

    fn tree_scale(&self) -> f32 {
        self.settings.tree_zoom / 0.65
    }

    fn tree_layout(&mut self) -> Arc<Vec<(Entry, egui::Pos2)>> {
        if let Some(layout) = &self.tree_layout {
            if layout.sort == self.settings.tree_sort
                && layout.descending == self.settings.tree_descending
                && layout.zoom == self.settings.tree_zoom
                && layout.expanded == self.expanded
            {
                return Arc::clone(&layout.items);
            }
        }
        let mut items = Vec::new();
        self.tree_items(self.display_entries().as_ref().clone(), 0, &mut items);
        let items = Arc::new(items);
        self.tree_layout = Some(TreeLayout {
            sort: self.settings.tree_sort,
            descending: self.settings.tree_descending,
            zoom: self.settings.tree_zoom,
            expanded: self.expanded.clone(),
            items: Arc::clone(&items),
        });
        items
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
            let y = items.len() as f32 * 32.0 * self.tree_scale();
            items.push((entry, egui::pos2(depth as f32, y)));
            if self.expanded.contains(&path) && depth < 64 {
                let preview = self
                    .new_folder
                    .as_ref()
                    .filter(|dialog| dialog.parent == path)
                    .and_then(|dialog| dialog.preview.as_ref());
                if let Some(children) = preview
                    .map(AsRef::as_ref)
                    .or_else(|| self.tree_cache.get(&path))
                {
                    self.tree_items(children.clone(), depth + 1, items);
                }
            }
        }
    }

    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        items: &[(Entry, egui::Pos2)],
        start: usize,
        context: &egui::Context,
    ) {
        let gutter = self.number_width();
        let current_row = self
            .visible_paths
            .iter()
            .position(|path| self.selected.as_ref() == Some(path))
            .unwrap_or(0);
        for (index, (entry, position)) in items.iter().enumerate() {
            let depth = position.x;
            ui.push_id(&entry.path, |ui| {
                let scale = self.tree_scale();
                let height = 32.0 * scale;
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
                    if (start + index) % 2 == 0 {
                        ui.visuals().panel_fill
                    } else {
                        ui.visuals().faint_bg_color
                    },
                );
                self.paint_relative_number(
                    ui,
                    egui::Rect::from_min_size(row.min, Vec2::new(gutter, row.height())),
                    start + index,
                    current_row,
                );
                let left = row.left() + gutter + depth * 20.0 * scale;
                let disclosure = egui::Rect::from_center_size(
                    egui::pos2(left + 14.0 * scale, row.center().y),
                    Vec2::splat(24.0 * scale),
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
                let content =
                    egui::Rect::from_min_max(egui::pos2(left + 30.0 * scale, row.top()), row.max);
                let name_width = ui
                    .painter()
                    .layout_no_wrap(
                        entry.name.clone(),
                        egui::FontId::proportional(14.0 * scale),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x;
                let hit_rect = egui::Rect::from_min_max(
                    content.min,
                    egui::pos2(
                        (content.left() + 44.0 * scale + name_width).min(content.right()),
                        content.bottom(),
                    ),
                );
                let response = ui.interact(
                    hit_rect,
                    ui.id().with("entry"),
                    egui::Sense::click_and_drag(),
                );
                paint_selection(
                    ui,
                    content,
                    !self.sidebar_focused && self.selection.contains(&entry.path),
                    false,
                );
                let edge = 22.0 * scale;
                let icon = egui::Rect::from_center_size(
                    egui::pos2(content.left() + edge / 2.0 + 6.0 * scale, row.center().y),
                    Vec2::splat(edge),
                );
                if !self.paint_thumbnail(ui, icon, entry) {
                    self.paint_entry_icon(ui, icon, entry);
                }
                if response.hovered() {
                    ui.painter().rect_filled(
                        content,
                        0.0,
                        ui.visuals().panel_fill.gamma_multiply(0.18),
                    );
                }
                let name_left = icon.right() + 10.0 * scale;
                let date_left = row.right() - TREE_DATE_WIDTH;
                let name_right = date_left - TREE_SIZE_WIDTH;
                let name_rect = egui::Rect::from_min_size(
                    egui::pos2(name_left, row.top() + 4.0),
                    Vec2::new((name_right - name_left - 8.0).max(1.0), row.height() - 8.0),
                );
                if !self.rename_field(ui, name_rect, entry, context) {
                    paint_name(
                        ui,
                        &entry.name,
                        egui::pos2(name_left, row.center().y),
                        name_rect.width(),
                        14.0 * scale,
                        if response.hovered() {
                            ACCENT
                        } else {
                            ui.visuals().text_color()
                        },
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
                ui.painter()
                    .with_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(name_right, row.top()),
                        egui::pos2(date_left, row.bottom()),
                    ))
                    .text(
                        egui::pos2(date_left - 12.0, row.center().y),
                        egui::Align2::RIGHT_CENTER,
                        size,
                        egui::FontId::proportional(11.0 * scale),
                        MUTED,
                    );
                ui.painter()
                    .with_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(date_left, row.top()),
                        row.max,
                    ))
                    .text(
                        egui::pos2(date_left + 8.0, row.center().y),
                        egui::Align2::LEFT_CENTER,
                        &entry.modified_label,
                        egui::FontId::proportional(11.0 * scale),
                        MUTED,
                    );
                self.entry_response(response, entry, context);
            });
        }
    }

    fn select_restored_paths(&mut self, paths: &[PathBuf], context: &egui::Context) {
        let paths: HashSet<_> = paths.iter().collect();
        let restored: HashSet<_> =
            self.entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .filter(|entry| {
                    paths.contains(&entry.path)
                        || entry.sequence.as_ref().is_some_and(|sequence| {
                            sequence.iter().any(|path| paths.contains(path))
                        })
                })
                .map(|entry| entry.path.clone())
                .collect();
        if !restored.is_empty() {
            self.select(restored.iter().min().cloned(), context);
            self.selection = restored;
        }
    }

    fn update_operation_visibility(
        &mut self,
        paths: &[PathBuf],
        restoring: bool,
        context: &egui::Context,
    ) {
        let roots: HashSet<_> = paths.iter().map(PathBuf::as_path).collect();
        let hidden = |path: &Path| path.ancestors().any(|root| roots.contains(root));
        let affected = |entry: &Entry| {
            hidden(&entry.path)
                || entry.sequence.as_ref().is_some_and(|sequence| {
                    sequence.iter().any(|path| roots.contains(path.as_path()))
                })
        };
        if restoring {
            let entries: Vec<_> = self
                .removed_entries
                .values()
                .filter(|entry| affected(entry))
                .cloned()
                .collect();
            for entry in entries {
                let Some(parent) = entry.path.parent() else {
                    continue;
                };
                let target = if parent == self.current
                    || (self.settings.recursive() && entry.path.starts_with(&self.current))
                {
                    Arc::make_mut(&mut self.entries)
                } else if let Some(entries) = self.tree_cache.get_mut(parent) {
                    entries
                } else {
                    continue;
                };
                if !target.iter().any(|existing| existing.path == entry.path) {
                    target.push(entry);
                }
            }
            Arc::make_mut(&mut self.entries).sort_by_cached_key(|entry| {
                (
                    !entry.directory,
                    entry.name.to_lowercase(),
                    entry.name.clone(),
                )
            });
            if let Some(order) = self.settings.entry_order.get(&self.current) {
                order_entries(Arc::make_mut(&mut self.entries).as_mut_slice(), order);
            }
            self.select_restored_paths(paths, context);
        } else {
            let entries_changed = self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .any(affected);
            let search_changed = self
                .search
                .as_ref()
                .and_then(|search| search.update.as_ref())
                .is_some_and(|update| update.hits.iter().any(|hit| hidden(&hit.path)));
            if !entries_changed && !search_changed {
                return;
            }
            let nearby = self.selected.as_ref().and_then(|selected| {
                let index = self
                    .visible_paths
                    .iter()
                    .position(|path| path == selected)?;
                let mut previous = self.visible_paths[..=index].iter().rev();
                let mut next = self.visible_paths[index + 1..].iter();
                if self.settings.view == View::Media {
                    next.find(|path| !hidden(path))
                        .or_else(|| previous.find(|path| !hidden(path)))
                } else {
                    previous
                        .find(|path| !hidden(path))
                        .or_else(|| next.find(|path| !hidden(path)))
                }
                .cloned()
            });
            for entry in self
                .entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .filter(|entry| affected(entry))
            {
                self.removed_entries
                    .insert(entry.path.clone(), entry.clone());
            }
            Arc::make_mut(&mut self.entries).retain(|entry| !affected(entry));
            for entries in self.tree_cache.values_mut() {
                entries.retain(|entry| !affected(entry));
            }
            self.visible_paths.retain(|path| !hidden(path));
            if let Some(search) = &mut self.search {
                if let Some(update) = &mut search.update {
                    update.hits.retain(|hit| !hidden(&hit.path));
                    search.cursor = search.cursor.min(update.hits.len().saturating_sub(1));
                }
            }
            self.tree_layout = None;
            self.retain_visible_selection(context);
            if self.selection.is_empty() {
                self.select(nearby, context);
            }
        }
        self.tree_layout = None;
        self.media_layout = Default::default();
        self.media_cache.clear();
        self.directory_frame = None;
        self.preparing_directory = false;
        self.wait_for_thumbnails = false;
    }

    fn move_paths(&mut self, paths: &[PathBuf], directory: &Path, context: &egui::Context) {
        if self.operation_job.is_some() || files::is_trash(directory) {
            return;
        }
        let paths = paths
            .iter()
            .filter(|path| path.parent() != Some(directory))
            .filter_map(|path| {
                path.file_name()
                    .map(|name| (path.clone(), directory.join(name)))
            })
            .collect();
        self.start_operation(FileOperation::Move(paths), context);
    }

    fn start_operation(&mut self, mut operation: FileOperation, context: &egui::Context) {
        if self.new_folder.is_some() {
            return;
        }
        if let FileOperation::Trash(paths) = &mut operation {
            if self
                .operation_job
                .as_ref()
                .is_some_and(|job| job.trash_paths.is_empty())
            {
                return;
            }
            self.mounts.poll();
            paths.retain(|path| {
                if matches!(self.mounts.kind(path), Some(mounts::Kind::Network)) {
                    if !self.pending_delete.contains(path) {
                        self.pending_delete.push(path.clone());
                    }
                    false
                } else {
                    true
                }
            });
            if paths.is_empty() {
                return;
            }
        }
        self.media_cache.clear();
        if let Some(job) = &mut self.operation_job {
            let FileOperation::Trash(mut paths) = operation else {
                return;
            };
            if job.trash_paths.is_empty() {
                return;
            }
            paths.retain(|path| !path.ancestors().any(|root| job.trash_paths.contains(root)));
            if paths.is_empty() {
                return;
            }
            job.total += paths.len();
            job.trash_paths.extend(paths.iter().cloned());
            let (sender, receiver) = mpsc::channel();
            job.pending_visibility.push(receiver);
            let queued = paths.clone();
            let context = context.clone();
            std::thread::spawn(move || {
                for chunk in queued.chunks(128) {
                    if files::can_trash(chunk) {
                        let _ = sender.send(chunk.to_vec());
                        context.request_repaint();
                    }
                }
            });
            job.pending_trash.extend(paths);
            return;
        }
        let previous_selection = matches!(operation, FileOperation::Trash(_)).then(|| {
            (
                self.current.clone(),
                DirectorySelection {
                    selected: self.selected.clone(),
                    last_selected: self.last_selected.clone(),
                    paths: self.selection.clone(),
                    anchor: self.selection_anchor.clone(),
                    visual_line: self.visual_line,
                },
            )
        });
        let label = match &operation {
            FileOperation::Rename(paths) if paths.is_empty() => return,
            FileOperation::Rename(_) => "Renaming",
            FileOperation::Move(paths) if paths.is_empty() => return,
            FileOperation::Move(_) => "Moving",
            FileOperation::Copy(paths, _) if paths.is_empty() => return,
            FileOperation::Copy(_, _) => "Copying",
            FileOperation::Trash(paths) if paths.is_empty() => return,
            FileOperation::Trash(_) => "Moving to trash",
            FileOperation::Delete(paths) if paths.is_empty() => return,
            FileOperation::Delete(_) => "Permanently deleting",
            FileOperation::EmptyTrash => "Emptying trash",
            FileOperation::Extract(_) => "Extracting",
            FileOperation::Undo => "Undoing",
            FileOperation::Redo => "Redoing",
        };
        let trash_paths: HashSet<_> = match &operation {
            FileOperation::Trash(paths) => paths.iter().cloned().collect(),
            _ => HashSet::new(),
        };
        let total = match &operation {
            FileOperation::Rename(paths) | FileOperation::Move(paths) => paths.len(),
            FileOperation::Copy(paths, _) => paths.len(),
            FileOperation::Delete(paths) => paths.len(),
            FileOperation::Extract(_) => 1,
            _ => trash_paths.len(),
        };
        let preserve_selection = matches!(operation, FileOperation::Undo | FileOperation::Redo);
        let history = std::mem::take(&mut self.operations);
        let receiver = Self::run_operation(history, operation, context);
        self.operation_job = Some(OperationJob {
            receiver,
            previous_selection,
            trash_paths,
            pending_trash: Vec::new(),
            completed_offset: 0,
            outcome: operations::Outcome::default(),
            preserve_selection,
            visual_changed: false,
            restored_paths: Vec::new(),
            hidden_paths: Vec::new(),
            pending_visibility: Vec::new(),
            label,
            completed: 0,
            total,
            path: PathBuf::new(),
        });
    }

    fn run_operation(
        mut history: operations::History,
        operation: FileOperation,
        context: &egui::Context,
    ) -> mpsc::Receiver<OperationUpdate> {
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        std::thread::spawn(move || {
            if let FileOperation::Trash(paths) = &operation {
                for chunk in paths.chunks(128) {
                    if files::can_trash(chunk) {
                        let _ = sender.send(OperationUpdate::Visibility(chunk.to_vec(), false));
                        context.request_repaint();
                    }
                }
            } else if let FileOperation::Delete(paths) = &operation {
                let _ = sender.send(OperationUpdate::Visibility(paths.clone(), false));
                context.request_repaint();
            }
            if matches!(operation, FileOperation::Undo | FileOperation::Redo) {
                let paths = history.restoring_paths(matches!(operation, FileOperation::Redo));
                if !paths.is_empty() {
                    let _ = sender.send(OperationUpdate::Visibility(paths, true));
                    context.request_repaint();
                }
            }
            let mut progress = |completed, total, path: &Path| {
                let _ = sender.send(OperationUpdate::Progress(
                    completed,
                    total,
                    path.to_path_buf(),
                ));
                context.request_repaint();
            };
            let outcome = match operation {
                FileOperation::Rename(paths) | FileOperation::Move(paths) => {
                    history.rename(paths, &mut progress)
                }
                FileOperation::Copy(paths, directory) => {
                    history.copy(paths, &directory, &mut progress)
                }
                FileOperation::Trash(paths) => history.trash(paths, &mut progress),
                FileOperation::Delete(paths) => history.delete(paths, &mut progress),
                FileOperation::EmptyTrash => history.empty_trash(&mut progress),
                FileOperation::Extract(path) => {
                    progress(0, 1, &path);
                    let result = files::extract(&path);
                    history.discard_redo();
                    progress(1, 1, &path);
                    operations::Outcome {
                        changed: true,
                        error: result
                            .err()
                            .map(|error| format!("Cannot extract {}: {error}", path.display())),
                        ..operations::Outcome::default()
                    }
                }
                FileOperation::Undo => history.undo(&mut progress),
                FileOperation::Redo => history.redo(&mut progress),
            };
            let _ = sender.send(OperationUpdate::Finished(history, outcome));
            context.request_repaint();
        });
        receiver
    }

    fn trash_selection(&mut self, context: &egui::Context) {
        if !files::is_trash(&self.current) {
            self.visual_line = false;
            self.start_operation(FileOperation::Trash(self.selected_trash_paths()), context);
        }
    }

    fn selected_trash_paths(&self) -> Vec<PathBuf> {
        let sequences: HashMap<_, _> = self
            .entries
            .iter()
            .chain(self.tree_cache.values().flatten())
            .filter_map(|entry| {
                entry
                    .sequence
                    .as_ref()
                    .map(|sequence| (&entry.path, sequence))
            })
            .collect();
        let mut paths = Vec::new();
        for path in &self.selection {
            if let Some(sequence) = sequences.get(path) {
                paths.extend(sequence.iter().cloned());
            } else {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        let selected: HashSet<_> = paths.iter().map(PathBuf::as_path).collect();
        paths
            .iter()
            .filter(|path| {
                !path
                    .ancestors()
                    .skip(1)
                    .any(|parent| selected.contains(parent))
            })
            .cloned()
            .collect()
    }

    fn nearby_selection(paths: &[PathBuf], selected: &Path) -> Option<PathBuf> {
        let index = paths.iter().position(|path| path == selected)?;
        paths[..=index]
            .iter()
            .rev()
            .chain(&paths[index + 1..])
            .find(|path| std::fs::symlink_metadata(path).is_ok())
            .cloned()
    }

    fn complete_operation(
        &mut self,
        outcome: operations::Outcome,
        preserve_selection: bool,
        context: &egui::Context,
    ) {
        for trashed in &outcome.trashed {
            for item in &mut self.copied_files {
                if item.path == trashed.path {
                    item.clone_from(trashed);
                }
            }
        }
        for (from, to) in &outcome.copied {
            for item in &mut self.copied_files {
                item.pasted(from, to);
            }
        }
        self.thumbnails.reuse_copies(&outcome.copied);
        self.thumbnails.reuse_renames(&outcome.renames);
        if let Some(mut placement) = self.paste_placement.take() {
            if !outcome.copied.is_empty() {
                placement.order.retain(|path| {
                    !outcome
                        .copied
                        .iter()
                        .any(|(_, destination)| destination == path)
                });
                let index = placement
                    .anchor
                    .as_ref()
                    .and_then(|anchor| placement.order.iter().position(|path| path == anchor))
                    .map_or(0, |index| index + 1);
                placement.order.splice(
                    index..index,
                    outcome
                        .copied
                        .iter()
                        .map(|(_, destination)| destination.clone()),
                );
                self.settings
                    .entry_order
                    .insert(placement.directory, placement.order);
                self.save();
            }
        }
        self.media_cache.clear();
        if outcome.changed {
            let nearby = self
                .selected
                .as_ref()
                .and_then(|selected| Self::nearby_selection(&self.visible_paths, selected));
            self.message = None;
            let renamed_selection = self.selected.as_ref().and_then(|selected| {
                outcome
                    .renames
                    .iter()
                    .find(|(from, _)| from == selected)
                    .map(|(_, to)| to.clone())
            });
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
            if !files::is_trash(&self.current) && !self.current.is_dir() {
                if let Some(parent) = self.current.ancestors().find(|path| path.is_dir()) {
                    self.navigate(parent.to_path_buf(), context);
                }
            } else {
                self.address = self.current.to_string_lossy().into_owned();
                self.refresh(context);
                self.preparing_directory = outcome.copied.is_empty()
                    && outcome.renames.is_empty()
                    && self.directory_frame.is_some();
                self.wait_for_thumbnails = self.preparing_directory;
            }
            let selected = if preserve_selection && outcome.renames.is_empty() {
                nearby.or(outcome.selected)
            } else {
                renamed_selection.or(outcome.selected).or(nearby)
            };
            let selected = selected
                .filter(|path| {
                    path.parent() == Some(self.current.as_path())
                        || ((self.settings.view == View::Tree || self.settings.recursive())
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
            if !preserve_selection && !outcome.copied.is_empty() {
                self.selection = outcome
                    .copied
                    .iter()
                    .map(|(_, destination)| destination)
                    .filter(|path| {
                        path.parent() == Some(self.current.as_path())
                            || (self.settings.view == View::Tree
                                && *path != &self.current
                                && path.starts_with(&self.current))
                    })
                    .cloned()
                    .collect();
                if self.selection.len() > 1 {
                    self.size_job = None;
                    self.selection_size = None;
                }
            }
            self.scroll_to_selection = true;
        }
        if outcome.error.is_some() {
            self.set_message(outcome.error);
        }
    }

    fn start_new_folder(&mut self, context: &egui::Context) {
        if self.operation_job.is_some() || files::is_trash(&self.current) {
            return;
        }
        let into_folder = self.settings.view == View::Tree
            && self
                .selected
                .as_ref()
                .is_some_and(|path| path.is_dir() && self.expanded.contains(path));
        let parent = if self.settings.view != View::Tree {
            self.current.clone()
        } else if into_folder {
            self.selected
                .clone()
                .unwrap_or_else(|| self.current.clone())
        } else {
            self.selected
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(&self.current)
                .to_path_buf()
        };
        let anchor = if into_folder {
            None
        } else {
            self.selected.clone()
        };
        let mut name = "New folder".to_owned();
        let mut index = 1;
        while parent.join(&name).symlink_metadata().is_ok() {
            name = format!("New folder ({index})");
            index += 1;
        }
        let path = parent.join(&name);
        let mut entries = if parent == self.current {
            self.entries.as_ref().clone()
        } else {
            self.tree_cache.get(&parent).cloned().unwrap_or_default()
        };
        if self.settings.view == View::Tree {
            self.sort_tree_entries(&mut entries);
        }
        entries.push(Entry {
            path: path.clone(),
            name: name.clone(),
            directory: true,
            symlink: false,
            deletion_locked: false,
            size: 0,
            item_count: Some(0),
            has_contents: Some(false),
            modified: None,
            modified_label: String::new(),
            sequence: None,
        });
        let previous_selection = DirectorySelection {
            selected: self.selected.clone(),
            last_selected: self.last_selected.clone(),
            paths: self.selection.clone(),
            anchor: self.selection_anchor.clone(),
            visual_line: self.visual_line,
        };
        self.select(None, context);
        self.selected = Some(path.clone());
        self.selection_anchor = Some(path.clone());
        self.selection.insert(path);
        self.scroll_to_selection = true;
        self.sidebar_focused = false;
        self.tree_layout = None;
        self.search = None;
        self.find_query = None;
        self.editing_path = false;
        self.new_folder = Some(NameDialog {
            preview: Some(Arc::new(entries)),
            previous_selection: Some(previous_selection),
            anchor,
            source: None,
            sources: Vec::new(),
            submit: false,
            parent,
            name,
            focus: true,
            error: None,
        });
        context.request_repaint();
    }

    fn cancel_name_edit(&mut self, context: &egui::Context) {
        if let Some(selection) = self
            .new_folder
            .take()
            .and_then(|dialog| dialog.previous_selection)
        {
            self.select(selection.selected, context);
            self.last_selected = selection.last_selected;
            self.selection = selection.paths;
            self.selection_anchor = selection.anchor;
            self.visual_line = selection.visual_line;
            if self.selection.len() > 1 {
                self.size_job = None;
                self.selection_size = None;
            }
            self.scroll_to_selection = self.selected.is_some();
            self.tree_layout = None;
        }
        context.request_repaint();
    }

    fn start_rename(&mut self) {
        if self.operation_job.is_some() || files::is_trash(&self.current) {
            return;
        }
        let Some(source) = self.selected.clone() else {
            return;
        };
        let sources = if self.selection.len() > 1 {
            self.visible_paths
                .iter()
                .filter(|path| self.selection.contains(*path))
                .filter(|path| {
                    !path
                        .ancestors()
                        .skip(1)
                        .any(|parent| self.selection.contains(parent))
                })
                .flat_map(|path| {
                    self.entries
                        .iter()
                        .chain(self.tree_cache.values().flatten())
                        .find(|entry| &entry.path == path)
                        .and_then(|entry| entry.sequence.as_ref())
                        .map_or_else(|| vec![path.clone()], |frames| frames.as_ref().clone())
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let source = if sources.is_empty() || sources.contains(&source) {
            source
        } else {
            sources[0].clone()
        };
        let (Some(parent), Some(name)) = (source.parent(), source.file_name()) else {
            return;
        };
        let Some(name) = name.to_str() else {
            self.set_message(Some("Cannot edit a name that is not valid UTF-8.".into()));
            return;
        };
        let name = if !sources.is_empty() {
            name.to_owned()
        } else {
            self.entries
                .iter()
                .chain(self.tree_cache.values().flatten())
                .find(|entry| entry.path == source && entry.sequence.is_some())
                .and_then(|_| files::sequence_rename_name(&source))
                .unwrap_or_else(|| name.to_owned())
        };
        self.visual_line = false;
        self.new_folder = Some(NameDialog {
            preview: None,
            previous_selection: None,
            anchor: None,
            parent: parent.to_path_buf(),
            name,
            source: Some(source),
            sources,
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
        let font_size = 14.0
            * if self.settings.view == View::Tree {
                self.tree_scale()
            } else {
                1.0
            };
        let Some(dialog) = self.new_folder.as_mut() else {
            return false;
        };
        let active = dialog.editing_path() == Some(&entry.path);
        if !active && !dialog.sources.contains(&entry.path) {
            return false;
        }
        let id = ui.make_persistent_id(("rename-entry", &entry.path));
        if !active {
            let mut preview = dialog.name.clone();
            ui.put(
                rect,
                egui::TextEdit::singleline(&mut preview)
                    .id(id)
                    .interactive(false)
                    .font(egui::FontId::proportional(font_size))
                    .vertical_align(egui::Align::Center)
                    .min_size(rect.size())
                    .margin(egui::Margin::symmetric(8, 0)),
            );
            return true;
        }
        let response = ui.put(
            rect,
            egui::TextEdit::singleline(&mut dialog.name)
                .id(id)
                .font(egui::FontId::proportional(font_size))
                .vertical_align(egui::Align::Center)
                .min_size(rect.size())
                .margin(egui::Margin::symmetric(8, 0)),
        );
        if dialog.focus {
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(context, id) {
                let length = if entry.directory {
                    dialog.name.chars().count()
                } else {
                    Path::new(&dialog.name)
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
        if response.changed() {
            context.request_repaint();
        }
        dialog.submit = (response.has_focus() || response.lost_focus())
            && ui.input(|input| input.key_pressed(Key::Enter));
        if response.lost_focus() && !dialog.submit {
            self.cancel_name_edit(context);
        }
        true
    }

    fn empty_trash_window(&mut self, context: &egui::Context) {
        let deleting = !self.pending_delete.is_empty();
        if !self.confirm_empty_trash && !deleting {
            self.trash_focus = Some(false);
            return;
        }
        let ready = self.operation_job.is_none();
        let mut focus = self.trash_focus.take();
        let activate = context.input_mut(|input| {
            if input.consume_key(egui::Modifiers::CTRL, Key::H)
                || input.consume_key(egui::Modifiers::NONE, Key::H)
            {
                focus = Some(false);
            }
            if input.consume_key(egui::Modifiers::CTRL, Key::L)
                || input.consume_key(egui::Modifiers::NONE, Key::L)
            {
                focus = Some(true);
            }
            input.consume_key(egui::Modifiers::NONE, Key::Enter)
                || input.consume_key(egui::Modifiers::CTRL, Key::M)
        });
        let mut confirm =
            ready && context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Y));
        let mut cancel = false;
        let frame = egui::Frame::window(&context.style())
            .inner_margin(20)
            .corner_radius(12)
            .shadow(egui::epaint::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: Color32::from_black_alpha(32),
            });
        let response = egui::Modal::new(egui::Id::new("empty-trash-confirmation"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(64))
            .show(context, |ui| {
                ui.set_width((context.screen_rect().width() - 64.0).clamp(160.0, 400.0));
                ui.label(
                    RichText::new(if deleting { "Delete Permanently?" } else { "Empty Trash?" })
                        .size(18.0)
                        .strong(),
                );
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        if deleting {
                            if self.pending_delete.len() > 1 {
                                ui.label(
                                    RichText::new(format!("{} selected items", self.pending_delete.len()))
                                        .small()
                                        .color(MUTED),
                                );
                                ui.add_space(4.0);
                            }
                            for path in self.pending_delete.iter().take(3) {
                                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                                ui.add(egui::Label::new(RichText::new(name).strong()).truncate())
                                    .on_hover_text(path.display().to_string());
                            }
                            if self.pending_delete.len() > 3 {
                                ui.label(
                                    RichText::new(format!("and {} more", self.pending_delete.len() - 3))
                                        .small()
                                        .color(MUTED),
                                );
                            }
                        } else {
                            ui.label(RichText::new("All items in the trash").strong());
                        }
                    });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(if deleting {
                        "These items could not be moved to the trash. Folders include their contents. This cannot be undone."
                    } else {
                        "All items will be permanently deleted. This cannot be undone."
                    })
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let active = &mut ui.visuals_mut().widgets.active;
                    active.weak_bg_fill = Color32::from_rgb(165, 69, 60);
                    active.fg_stroke.color = Color32::WHITE;
                    let confirm_button = ui
                        .add_enabled_ui(ready, |ui| {
                            ui.add_sized(
                                [178.0, 34.0],
                                egui::Button::new(if deleting { "Delete Permanently · Y" } else { "Empty Trash · Y" })
                                    .stroke(Stroke::NONE)
                                    .corner_radius(6),
                            )
                        })
                        .inner;
                    let cancel_button = ui.add_sized(
                        [112.0, 34.0],
                        egui::Button::new("Cancel · Esc")
                            .stroke(Stroke::NONE)
                            .corner_radius(6),
                    );
                    if focus == Some(false) {
                        cancel_button.request_focus();
                    } else if focus == Some(true) && ready {
                        confirm_button.request_focus();
                    }
                    confirm |= confirm_button.clicked()
                        || (ready && activate && confirm_button.has_focus());
                    cancel = cancel_button.clicked() || (activate && cancel_button.has_focus());
                });
            });
        if cancel || response.should_close() {
            self.confirm_empty_trash = false;
            self.pending_delete.clear();
        } else if confirm {
            self.confirm_empty_trash = false;
            let operation = if deleting {
                FileOperation::Delete(std::mem::take(&mut self.pending_delete))
            } else {
                FileOperation::EmptyTrash
            };
            self.start_operation(operation, context);
        }
    }

    fn submit_name_edit(&mut self, context: &egui::Context) {
        let create = self.new_folder.as_ref().is_some_and(|folder| folder.submit);
        if create && self.operation_job.is_none() {
            if let Some(folder) = &mut self.new_folder {
                let source = folder.source.clone();
                let sequence = source.as_ref().and_then(|source| {
                    self.entries
                        .iter()
                        .chain(self.tree_cache.values().flatten())
                        .find(|entry| &entry.path == source)
                        .and_then(|entry| entry.sequence.clone())
                });
                let rename_paths = if !folder.sources.is_empty() {
                    Some(files::batch_rename_paths(&folder.sources, &folder.name))
                } else {
                    sequence.map(|frames| files::sequence_rename_paths(&frames, &folder.name))
                };
                if let Some(result) = rename_paths {
                    match result {
                        Ok(paths) => {
                            if self.message == folder.error {
                                self.message = None;
                            }
                            self.new_folder = None;
                            self.start_operation(FileOperation::Rename(paths), context);
                        }
                        Err(error) => {
                            let error = format!("Cannot save name: {error}");
                            folder.error = Some(error.clone());
                            folder.focus = true;
                            folder.submit = false;
                            self.set_message(Some(error));
                        }
                    }
                    return;
                }
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
                            if let Some(order) = folder.creation_order(&path) {
                                self.settings
                                    .entry_order
                                    .insert(folder.parent.clone(), order);
                            }
                        }
                        self.new_folder = None;
                        self.save();
                        self.complete_operation(
                            operations::Outcome {
                                changed: true,
                                pending_delete: Vec::new(),
                                renames,
                                copied: Vec::new(),
                                trashed: Vec::new(),
                                selected: Some(path),
                                error: None,
                            },
                            false,
                            context,
                        );
                    }
                    Err(error) => {
                        let error = format!("Cannot save name: {error}");
                        folder.focus = true;
                        folder.submit = false;
                        folder.error = Some(error.clone());
                        self.set_message(Some(error));
                    }
                }
            }
        }
    }
}

impl eframe::App for FileManager {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        if context.input(|input| {
            input.raw.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            })
        }) {
            self.external_drag.cancel();
        }
        let frame_directory = self.current.clone();
        if self.settings.view != View::Media || self.search.is_some() {
            self.media_scroll_target = None;
            self.media_scrolling_until = 0.0;
        }
        self.directory_ready = true;
        if self.settings.view == View::Media && self.search.is_none() {
            self.preparing_directory = false;
            self.wait_for_thumbnails = false;
        }
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
        if let Err(error) = self.bookmarks.poll(context) {
            self.set_message(Some(format!("Cannot read shared bookmarks: {error}")));
        }
        self.poll(context);
        self.receive_external_files(context);
        if self.entry_menu.is_some() {
            self.entry_menu_shortcuts(context);
        } else if !self.confirm_empty_trash && self.pending_delete.is_empty() {
            self.shortcuts(context);
        }
        let previous_view = std::mem::take(&mut self.prepared_view);
        if self.search.is_some()
            || self.settings.view == View::Media
            || self.current != frame_directory
        {
            self.marquee_selection = None;
        }
        self.toolbar(context);
        self.statusbar(context);
        self.sidebar(context);
        if self.message.is_some() {
            let remaining = self
                .message_deadline
                .saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                self.message = None;
            } else {
                context.request_repaint_after(remaining);
            }
        }
        if self.message.is_some() {
            egui::TopBottomPanel::bottom("messages")
                .frame(egui::Frame::NONE.fill(context.style().visuals.panel_fill))
                .show(context, |ui| self.message_banner(ui));
        }
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
                if self.search.is_none() && self.settings.view == View::Tree {
                    self.tree_header(ui);
                }
                if self.search.is_some() {
                    self.search_results(ui, context);
                } else if self.display_entries().is_empty() {
                    ui.centered_and_justified(|ui| {
                        let message = if self.media_scan.is_some() {
                            "Scanning media…"
                        } else if self.directory_refresh.is_some() {
                            "Loading folder…"
                        } else if files::is_trash(&self.current) {
                            "Trash is empty"
                        } else if self.settings.recursive() {
                            "No media found"
                        } else {
                            "This folder is empty"
                        };
                        ui.label(RichText::new(message).color(MUTED));
                    });
                } else {
                    match self.settings.view {
                        View::Grid => self.grid(ui, context),
                        View::Media => self.media(ui, context),
                        View::Tree => {
                            let items = self.tree_layout();
                            let height = 32.0 * self.tree_scale();
                            let navigation: Vec<_> = items
                                .iter()
                                .map(|(entry, position)| (entry, *position, height))
                                .collect();
                            ui.scope(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                self.navigate_items(
                                    &navigation,
                                    ui,
                                    "tree",
                                    items.len() as f32 * height,
                                    context,
                                );
                                let mut scroll = self.entry_scroll_area(ui, "tree");
                                if self.scroll_to_selection {
                                    if let Some(index) = items.iter().position(|(entry, _)| {
                                        self.selected.as_ref() == Some(&entry.path)
                                    }) {
                                        let top = index as f32 * height;
                                        let offset = egui::scroll_area::State::load(
                                            context,
                                            ui.make_persistent_id(egui::Id::new("tree")),
                                        )
                                        .map_or(0.0, |state| state.offset.y);
                                        let viewport = ui.available_height();
                                        let offset = if self.center_selection {
                                            top + (height - viewport) / 2.0
                                        } else {
                                            offset.min(top).max(top + height - viewport)
                                        };
                                        let max_offset =
                                            (items.len() as f32 * height - viewport).max(0.0);
                                        scroll = scroll
                                            .vertical_scroll_offset(offset.clamp(0.0, max_offset));
                                    }
                                    self.scroll_to_selection = false;
                                    self.center_selection = false;
                                }
                                with_scrollbar_color(ui, |ui, content_style| {
                                    scroll
                                        .auto_shrink([false, false])
                                        .drag_to_scroll(false)
                                        .scroll_bar_visibility(
                                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                        )
                                        .show_rows(ui, height, items.len(), |ui, range| {
                                            ui.set_style(content_style);
                                            let background = ui.interact(
                                                ui.clip_rect(),
                                                ui.id().with("marquee"),
                                                egui::Sense::click_and_drag(),
                                            );
                                            let origin = ui.cursor().min
                                                - Vec2::new(0.0, range.start as f32 * height);
                                            let width = ui.available_width();
                                            self.tree(
                                                ui,
                                                &items[range.clone()],
                                                range.start,
                                                context,
                                            );
                                            let gutter = self.number_width();
                                            let scale = self.tree_scale();
                                            self.marquee_selection(
                                                ui,
                                                &background,
                                                origin,
                                                items.iter().enumerate().map(
                                                    |(index, (entry, position))| {
                                                        let left = gutter
                                                            + position.x * 20.0 * scale
                                                            + 30.0 * scale;
                                                        let rect = egui::Rect::from_min_size(
                                                            origin
                                                                + Vec2::new(
                                                                    left,
                                                                    index as f32 * height,
                                                                ),
                                                            Vec2::new(
                                                                (width - left).max(0.0),
                                                                height,
                                                            ),
                                                        );
                                                        (entry.path.clone(), rect)
                                                    },
                                                ),
                                            );
                                        });
                                });
                            });
                        }
                    }
                }
                self.directory_background(&background, context);
                self.show_entry_menu(ui, context);
            });
        if !context.input(|input| input.pointer.primary_down()) {
            self.marquee_selection = None;
        }
        self.submit_name_edit(context);
        self.empty_trash_window(context);
        if self.current != frame_directory
            || !self.directory_ready
            || ((self.settings.view != View::Media || self.search.is_some())
                && self.preparing_directory
                && (self.directory_refresh.is_some() || self.prepared_view != previous_view))
        {
            context.request_repaint_after(std::time::Duration::from_millis(16));
            if let Some(frame) = &self.directory_frame {
                self.preparing_directory = true;
                context.graphics_mut(|graphics| graphics.clone_from(&frame.graphics));
                return;
            }
        }
        self.preparing_directory = false;
        self.wait_for_thumbnails = false;
        self.directory_frame = Some(DirectoryFrame {
            graphics: context.graphics(Clone::clone),
            _textures: self
                .icons
                .textures()
                .chain(self.thumbnails.textures())
                .cloned()
                .collect(),
        });
        if egui::DragAndDrop::has_payload_of_type::<Vec<PathBuf>>(context) {
            context.set_cursor_icon(egui::CursorIcon::Grabbing);
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
    response
}

fn hidden_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").selected(selected));
    let center = response.rect.center();
    let stroke = Stroke::new(1.5, ui.visuals().text_color());
    let outline = [
        Vec2::new(-9.0, 0.0),
        Vec2::new(-5.0, -5.0),
        Vec2::new(0.0, -6.0),
        Vec2::new(5.0, -5.0),
        Vec2::new(9.0, 0.0),
        Vec2::new(5.0, 5.0),
        Vec2::new(0.0, 6.0),
        Vec2::new(-5.0, 5.0),
    ];
    ui.painter().add(egui::Shape::closed_line(
        outline.into_iter().map(|offset| center + offset).collect(),
        stroke,
    ));
    ui.painter().circle_stroke(center, 2.5, stroke);
    response.on_hover_text("Hidden")
}

fn sequences_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").selected(selected));
    let center = response.rect.center();
    let stroke = Stroke::new(1.5, ui.visuals().text_color());
    for offset in [-3.0, 0.0, 3.0] {
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(center + Vec2::splat(offset), Vec2::new(12.0, 10.0)),
            1.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_text("Sequences · S")
}

fn recursive_media_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let response = ui.add_sized([32.0, 32.0], egui::Button::new("").selected(selected));
    let center = response.rect.center();
    let stroke = Stroke::new(1.5, ui.visuals().text_color());
    ui.painter().line_segment(
        [
            center + Vec2::new(-8.0, -8.0),
            center + Vec2::new(-8.0, 8.0),
        ],
        stroke,
    );
    for y in [-6.0, 6.0] {
        ui.painter().line_segment(
            [center + Vec2::new(-8.0, y), center + Vec2::new(-3.0, y)],
            stroke,
        );
    }
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            center + Vec2::new(0.0, -7.0),
            center + Vec2::new(9.0, 0.0),
            center + Vec2::new(0.0, 7.0),
        ],
        ui.visuals().text_color(),
        Stroke::NONE,
    ));
    response.on_hover_text("Recursive media · R")
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
    response
}

fn theme_button(ui: &mut egui::Ui) -> egui::Response {
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
    response
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

fn order_entries(entries: &mut [Entry], order: &[PathBuf]) {
    let positions: HashMap<_, _> = order
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_path(), index))
        .collect();
    entries.sort_by_key(|entry| {
        positions
            .get(entry.path.as_path())
            .copied()
            .unwrap_or(usize::MAX)
    });
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

fn paint_name(
    ui: &egui::Ui,
    name: &str,
    center: egui::Pos2,
    width: f32,
    size: f32,
    color: Color32,
) {
    let mut job = egui::text::LayoutJob::simple(
        name.into(),
        egui::FontId::proportional(size),
        color,
        width.max(1.0),
    );
    job.wrap.max_rows = 1;
    let galley = ui.painter().layout_job(job);
    let position = egui::pos2(center.x, center.y - galley.size().y / 2.0);
    ui.painter().galley(position, galley, color);
}

fn paint_media_badge(ui: &egui::Ui, rect: egui::Rect, entry: &Entry) {
    if entry.directory || !entry.animated() || rect.width() < 32.0 || rect.height() < 32.0 {
        return;
    }
    let sequence = entry.sequence.as_ref();
    let label = sequence.map_or_else(
        || "Video".to_owned(),
        |frames| format!("{} frames", frames.len()),
    );
    let color = if sequence.is_some() {
        Color32::from_rgb(163, 230, 200)
    } else {
        Color32::from_rgb(179, 211, 255)
    };
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let text_color = Color32::from_rgb(245, 247, 250);
    let galley = painter.layout_no_wrap(label, egui::FontId::proportional(11.0), text_color);
    let show_label = galley.size().x + 40.0 <= rect.width();
    let width = if show_label {
        galley.size().x + 30.0
    } else {
        22.0
    };
    let badge = egui::Rect::from_min_size(rect.min + Vec2::splat(6.0), Vec2::new(width, 22.0));
    painter.rect_filled(badge, 6.0, Color32::from_black_alpha(200));
    painter.rect_stroke(
        badge,
        6.0,
        egui::Stroke::new(1.0, Color32::from_white_alpha(40)),
        egui::StrokeKind::Inside,
    );
    let center = egui::pos2(badge.left() + 11.0, badge.center().y);
    if sequence.is_some() {
        let back = egui::Rect::from_center_size(center + Vec2::new(1.5, -1.5), Vec2::new(9.0, 7.0));
        painter.rect_stroke(
            back,
            1.5,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
        let front = back.translate(Vec2::new(-3.0, 3.0));
        painter.rect_filled(front, 1.5, Color32::from_rgb(26, 35, 33));
        painter.rect_stroke(
            front,
            1.5,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + Vec2::new(-3.0, -4.5),
                center + Vec2::new(4.0, 0.0),
                center + Vec2::new(-3.0, 4.5),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
    if show_label {
        painter.galley(
            egui::pos2(
                badge.left() + 23.0,
                badge.center().y - galley.size().y / 2.0,
            ),
            galley,
            text_color,
        );
    }
}

fn texture_tint(ui: &egui::Ui, texture: &egui::TextureHandle) -> Color32 {
    let opacity = ui.ctx().animate_bool_with_time(
        egui::Id::new(("loaded-texture", texture.id())),
        true,
        0.12,
    );
    Color32::WHITE.gamma_multiply(opacity)
}

fn paint_thumbnail_placeholder(ui: &egui::Ui, rect: egui::Rect) {
    let (background, border, foreground) = if ui.visuals().dark_mode {
        (
            Color32::from_gray(44),
            Color32::from_gray(54),
            Color32::from_gray(116),
        )
    } else {
        (
            Color32::from_gray(232),
            Color32::from_gray(220),
            Color32::from_gray(156),
        )
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, background);
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    let shortest = rect.width().min(rect.height());
    let edge = (shortest * 0.28).clamp(12.0, 44.0).min(shortest * 0.7);
    let icon = egui::Rect::from_center_size(rect.center(), Vec2::new(edge, edge * 0.8));
    let stroke = Stroke::new((edge / 24.0).clamp(1.0, 1.8), foreground);
    let point = |x, y| icon.min + icon.size() * Vec2::new(x, y);
    painter.rect_stroke(icon, 3.0, stroke, egui::StrokeKind::Inside);
    painter.circle_filled(point(0.7, 0.3), edge * 0.065, foreground);
    painter.add(egui::Shape::line(
        vec![
            point(0.12, 0.78),
            point(0.36, 0.44),
            point(0.55, 0.68),
            point(0.68, 0.53),
            point(0.88, 0.78),
        ],
        stroke,
    ));
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

fn paint_folder_location(ui: &egui::Ui, rect: egui::Rect, location: Option<mounts::Kind>) {
    let Some(location) = location else {
        return;
    };
    let edge = (rect.width().min(rect.height()) * 0.45).clamp(11.0, 22.0);
    let badge =
        egui::Rect::from_min_size(rect.right_top() - Vec2::new(edge, 0.0), Vec2::splat(edge));
    let painter = ui.painter();
    let color = match location {
        mounts::Kind::Network => Color32::from_rgb(57, 112, 176),
        mounts::Kind::External => Color32::from_rgb(160, 108, 42),
    };
    painter.rect_filled(badge, edge * 0.18, color);
    let point = |x, y| badge.min + badge.size() * Vec2::new(x, y);
    let stroke = Stroke::new((edge * 0.08).max(1.0), Color32::WHITE);
    match location {
        mounts::Kind::Network => {
            for (start, end) in [
                ((0.5, 0.35), (0.5, 0.62)),
                ((0.25, 0.62), (0.75, 0.62)),
                ((0.25, 0.62), (0.25, 0.76)),
                ((0.75, 0.62), (0.75, 0.76)),
            ] {
                painter.line_segment([point(start.0, start.1), point(end.0, end.1)], stroke);
            }
            for (x, y) in [(0.5, 0.28), (0.25, 0.78), (0.75, 0.78)] {
                painter.rect_filled(
                    egui::Rect::from_center_size(point(x, y), Vec2::splat(edge * 0.23)),
                    edge * 0.03,
                    Color32::WHITE,
                );
            }
        }
        mounts::Kind::External => {
            let drive = egui::Rect::from_min_max(point(0.18, 0.23), point(0.82, 0.78));
            painter.rect_stroke(drive, edge * 0.08, stroke, egui::StrokeKind::Inside);
            painter.line_segment([point(0.25, 0.57), point(0.75, 0.57)], stroke);
            painter.circle_filled(point(0.66, 0.68), edge * 0.045, Color32::WHITE);
        }
    }
}

fn paint_deletion_lock(ui: &egui::Ui, rect: egui::Rect, locked: bool) {
    if !locked || !ui.is_rect_visible(rect) {
        return;
    }
    let edge = (rect.width().min(rect.height()) * 0.4).clamp(12.0, 22.0);
    let badge =
        egui::Rect::from_min_size(rect.right_bottom() - Vec2::splat(edge), Vec2::splat(edge));
    let painter = ui.painter();
    painter.rect_filled(badge, edge * 0.2, ui.visuals().panel_fill);
    let color = ui.visuals().text_color();
    let shackle = egui::Rect::from_min_max(
        badge.min + Vec2::new(edge * 0.32, edge * 0.16),
        badge.min + Vec2::new(edge * 0.68, edge * 0.62),
    );
    painter.rect_stroke(
        shackle,
        edge * 0.18,
        Stroke::new((edge * 0.09).max(1.2), color),
        egui::StrokeKind::Inside,
    );
    let body = egui::Rect::from_min_max(
        badge.min + Vec2::new(edge * 0.2, edge * 0.44),
        badge.min + Vec2::new(edge * 0.8, edge * 0.86),
    );
    painter.rect_filled(body, edge * 0.08, color);
    painter.circle_filled(body.center(), edge * 0.06, ui.visuals().panel_fill);
}

fn paint_icon(ui: &egui::Ui, rect: egui::Rect, entry: &Entry) {
    let painter = ui.painter();
    if entry.directory {
        paint_folder(ui, rect);
        if entry
            .has_contents
            .unwrap_or_else(|| entry.item_count.is_some_and(|count| count > 0))
        {
            for (left, top, right, bottom, color) in [
                (0.3, 0.38, 0.62, 0.72, Color32::from_rgb(214, 225, 220)),
                (0.4, 0.46, 0.72, 0.8, Color32::from_rgb(244, 248, 246)),
            ] {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        rect.min + rect.size() * Vec2::new(left, top),
                        rect.min + rect.size() * Vec2::new(right, bottom),
                    ),
                    1.0,
                    color,
                );
            }
        }
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

struct ResidentApp {
    root: FileManager,
    icon: Arc<egui::IconData>,
    visible: bool,
    quitting: bool,
    windows: Vec<(egui::ViewportId, FileManager)>,
    next_window: u64,
    requests: mpsc::Receiver<instance::Request>,
    _instance: instance::Instance,
}

impl eframe::App for ResidentApp {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        if self.quitting {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        while let Ok(request) = self.requests.try_recv() {
            match request {
                instance::Request::Quit => {
                    self.quitting = true;
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                instance::Request::NavigateStill(path, forward) => {
                    if !self.root.navigate_still(&path, forward, context) {
                        for (_, window) in &mut self.windows {
                            if window.navigate_still(&path, forward, context) {
                                break;
                            }
                        }
                    }
                }
                instance::Request::Open(path) => {
                    if self.visible {
                        let id = egui::ViewportId::from_hash_of(("file-manager", self.next_window));
                        self.next_window += 1;
                        let window =
                            FileManager::new(context, self.root.external_drag.sibling(), path);
                        self.windows.push((id, window));
                    } else {
                        let directory = path
                            .or_else(|| {
                                std::fs::read(&self.root.config)
                                    .ok()
                                    .and_then(|bytes| {
                                        serde_json::from_slice::<Settings>(&bytes).ok()
                                    })
                                    .and_then(|settings| settings.last_directory)
                            })
                            .unwrap_or_else(|| self.root.current.clone());
                        self.root.navigate(directory, context);
                        self.root.refresh(context);
                        self.visible = true;
                        context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        context.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
            }
        }
        if context.input(|input| input.viewport().close_requested()) {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.root.external_drag.cancel();
            self.visible = false;
        }
        if self.visible {
            apply_theme(context, self.root.settings.dark_mode);
            self.root.update(context, frame);
        }
        self.windows.retain_mut(|(id, window)| {
            let mut closed = false;
            context.show_viewport_immediate(
                *id,
                egui::ViewportBuilder::default()
                    .with_title("Still Files")
                    .with_inner_size([1120.0, 740.0])
                    .with_min_inner_size([760.0, 460.0])
                    .with_icon(self.icon.clone()),
                |context, _| {
                    closed = context.input(|input| input.viewport().close_requested());
                    if !closed {
                        apply_theme(context, window.settings.dark_mode);
                        window.update(context, frame);
                    }
                },
            );
            !closed
        });
    }
}

fn main() -> eframe::Result {
    let argument = std::env::args_os().nth(1);
    let request = if argument.as_deref() == Some(std::ffi::OsStr::new("--quit")) {
        instance::Request::Quit
    } else {
        let path = argument.map(PathBuf::from).map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        });
        instance::Request::Open(path)
    };
    let Some(instance) = instance::Instance::start(&request)
        .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?
    else {
        return Ok(());
    };
    let instance::Request::Open(path) = request else {
        return Ok(());
    };
    let icon = Arc::new(
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
            .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?,
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([760.0, 460.0])
            .with_icon(icon.clone()),
        ..Default::default()
    };
    eframe::run_native(
        "Still Files",
        options,
        Box::new(move |context| {
            let requests = instance.listen(context.egui_ctx.clone())?;
            let root = FileManager::new(&context.egui_ctx, drag::ExternalDrag::new(context), path);
            Ok(Box::new(ResidentApp {
                root,
                icon,
                visible: true,
                quitting: false,
                windows: Vec::new(),
                next_window: 1,
                requests,
                _instance: instance,
            }))
        }),
    )
}
