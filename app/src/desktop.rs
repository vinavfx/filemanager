use crate::{drag, places, FileManager};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, ImageFormat, ImageOrder, Window,
};
use x11rb::rust_connection::RustConnection;

const TITLE: &str = "Still Files Desktop";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _XROOTPMAP_ID,
        ESETROOT_PMAP_ID,
        _NET_CLIENT_LIST,
        _NET_WM_NAME,
        UTF8_STRING,
        _NET_WM_PID,
        _NET_CURRENT_DESKTOP,
        _NET_WORKAREA,
        _NET_WM_DESKTOP,
        _NET_WM_STATE,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_BELOW,
    }
}

pub fn check_session() -> std::result::Result<(), &'static str> {
    if std::env::var_os("DISPLAY").is_none()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        Err("Desktop mode requires an X11 session")
    } else {
        Ok(())
    }
}

fn preference_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config")
        })
        .join("still-files/desktop-enabled")
}

pub fn is_enabled() -> bool {
    preference_path().is_file()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = preference_path();
    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, [])?;
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub struct Desktop {
    manager: FileManager,
    connection: RustConnection,
    atoms: Atoms,
    root: Window,
    geometry: [u32; 4],
    configured: bool,
    wallpaper_source: Option<u32>,
    wallpaper: Option<egui::TextureHandle>,
    wallpaper_job: Option<mpsc::Receiver<std::result::Result<egui::ColorImage, String>>>,
    next_refresh: Instant,
}

impl Desktop {
    pub fn new(context: &egui::Context, external_drag: drag::ExternalDrag) -> Result<Self> {
        check_session()?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?;
        let path = places::defaults(&home)
            .into_iter()
            .find(|place| matches!(place.kind, places::Kind::Desktop))
            .map(|place| place.path)
            .ok_or("The desktop folder does not exist or is disabled")?;
        let (connection, screen) = x11rb::connect(None)?;
        let atoms = Atoms::new(&connection)?.reply()?;
        let screen = &connection.setup().roots[screen];
        let root = screen.root;
        let geometry = [
            0,
            0,
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        ];
        Ok(Self {
            manager: FileManager::new_with_mode(context, external_drag, Some(path), true),
            connection,
            atoms,
            root,
            geometry,
            configured: false,
            wallpaper_source: None,
            wallpaper: None,
            wallpaper_job: None,
            next_refresh: Instant::now(),
        })
    }

    fn refresh(&mut self, context: &egui::Context) -> Result<()> {
        if self.wallpaper_job.is_none() {
            self.refresh_wallpaper(context)?;
        }
        let current = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_CURRENT_DESKTOP,
                AtomEnum::CARDINAL,
                0,
                1,
            )?
            .reply()?
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0);
        let area = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_WORKAREA,
                AtomEnum::CARDINAL,
                current.saturating_mul(4),
                4,
            )?
            .reply()?;
        if let Some(values) = area.value32() {
            let values: Vec<_> = values.collect();
            if let Ok(rect) = <[u32; 4]>::try_from(values) {
                if rect[2] > 0 && rect[3] > 0 {
                    self.geometry = rect;
                }
            }
        }
        if self.configured {
            return Ok(());
        }
        let clients = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_CLIENT_LIST,
                AtomEnum::WINDOW,
                0,
                4096,
            )?
            .reply()?;
        for window in clients.value32().into_iter().flatten() {
            let title = self
                .connection
                .get_property(
                    false,
                    window,
                    self.atoms._NET_WM_NAME,
                    self.atoms.UTF8_STRING,
                    0,
                    64,
                )?
                .reply()?;
            if title.value != TITLE.as_bytes() {
                continue;
            }
            let pid = self
                .connection
                .get_property(
                    false,
                    window,
                    self.atoms._NET_WM_PID,
                    AtomEnum::CARDINAL,
                    0,
                    1,
                )?
                .reply()?
                .value32()
                .and_then(|mut values| values.next());
            if pid != Some(std::process::id()) {
                continue;
            }
            self.send(window, self.atoms._NET_WM_DESKTOP, [u32::MAX, 1, 0, 0, 0])?;
            for (first, second) in [
                (
                    self.atoms._NET_WM_STATE_SKIP_TASKBAR,
                    self.atoms._NET_WM_STATE_SKIP_PAGER,
                ),
                (
                    self.atoms._NET_WM_STATE_STICKY,
                    self.atoms._NET_WM_STATE_BELOW,
                ),
            ] {
                self.send(window, self.atoms._NET_WM_STATE, [1, first, second, 1, 0])?;
            }
            self.connection.flush()?;
            self.configured = true;
            break;
        }
        Ok(())
    }

    fn refresh_wallpaper(&mut self, context: &egui::Context) -> Result<()> {
        let mut source = None;
        for atom in [self.atoms._XROOTPMAP_ID, self.atoms.ESETROOT_PMAP_ID] {
            source = self
                .connection
                .get_property(false, self.root, atom, AtomEnum::PIXMAP, 0, 1)?
                .reply()?
                .value32()
                .and_then(|mut values| values.next())
                .filter(|pixmap| *pixmap != 0);
            if source.is_some() {
                break;
            }
        }
        if source == self.wallpaper_source {
            return Ok(());
        }
        self.wallpaper_source = source;
        let Some(pixmap) = source else {
            self.wallpaper = None;
            return Ok(());
        };
        let (sender, receiver) = mpsc::channel();
        let max_side = context.input(|input| input.max_texture_side);
        let context = context.clone();
        std::thread::spawn(move || {
            let image = load_wallpaper(pixmap, max_side).map_err(|error| error.to_string());
            let _ = sender.send(image);
            context.request_repaint();
        });
        self.wallpaper_job = Some(receiver);
        Ok(())
    }

    fn send(&self, window: Window, atom: u32, data: [u32; 5]) -> Result<()> {
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, window, atom, data),
            )?
            .check()?;
        Ok(())
    }

    pub fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        if Instant::now() >= self.next_refresh {
            if let Err(error) = self.refresh(context) {
                eprintln!("Cannot update desktop window: {error}");
            }
            self.next_refresh = Instant::now() + Duration::from_secs(1);
        }
        if let Some(result) = self
            .wallpaper_job
            .as_ref()
            .and_then(|job| job.try_recv().ok())
        {
            self.wallpaper_job = None;
            match result {
                Ok(image) => {
                    self.wallpaper = Some(context.load_texture(
                        "desktop-wallpaper",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                Err(error) => eprintln!("Cannot load desktop wallpaper: {error}"),
            }
        }
        let scale = context.native_pixels_per_point().unwrap_or(1.0);
        let [x, y, width, height] = self.geometry.map(|value| value as f32 / scale);
        context.show_viewport_immediate(
            egui::ViewportId::from_hash_of("still-files-desktop"),
            egui::ViewportBuilder::default()
                .with_title(TITLE)
                .with_window_type(egui::X11WindowType::Desktop)
                .with_window_level(egui::WindowLevel::AlwaysOnBottom)
                .with_decorations(false)
                .with_taskbar(false)
                .with_active(false)
                .with_resizable(false)
                .with_position([x, y])
                .with_inner_size([width, height]),
            |context, _| {
                if context.input(|input| input.viewport().close_requested()) {
                    context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                crate::apply_theme(context, self.manager.settings.dark_mode);
                let painter = context.layer_painter(egui::LayerId::background());
                let rect = context.screen_rect();
                painter.rect_filled(rect, 0.0, context.style().visuals.panel_fill);
                if let Some(wallpaper) = &self.wallpaper {
                    let size = wallpaper.size_vec2();
                    let [left, top, width, height] = self.geometry.map(|value| value as f32);
                    let uv = egui::Rect::from_min_max(
                        egui::pos2(left / size.x, top / size.y),
                        egui::pos2((left + width) / size.x, (top + height) / size.y),
                    );
                    painter.image(wallpaper.id(), rect, uv, egui::Color32::WHITE);
                }
                eframe::App::update(&mut self.manager, context, frame);
            },
        );
    }
}

pub fn set_wallpaper(path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    let path = path
        .to_str()
        .ok_or("The wallpaper path must be valid UTF-8")?;
    let value = format!("'{}'", path.replace('\\', "\\\\").replace('\'', "\\'"));
    for (key, value) in [
        ("picture-filename", value.as_str()),
        ("picture-options", "'zoom'"),
        ("draw-background", "true"),
    ] {
        let output = std::process::Command::new("gsettings")
            .args(["set", "org.mate.background", key, value])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "Cannot set desktop wallpaper: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
    }
    Ok(())
}

fn load_wallpaper(pixmap: u32, max_side: usize) -> Result<egui::ColorImage> {
    let (connection, screen) = x11rb::connect(None)?;
    let geometry = connection.get_geometry(pixmap)?.reply()?;
    let width = usize::from(geometry.width);
    let height = usize::from(geometry.height);
    if width == 0 || height == 0 || width > max_side || height > max_side {
        return Err("The wallpaper exceeds the supported texture size".into());
    }
    let setup = connection.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == geometry.depth)
        .ok_or("Unsupported wallpaper depth")?;
    if !matches!(format.bits_per_pixel, 16 | 24 | 32) || format.scanline_pad == 0 {
        return Err("Unsupported wallpaper pixel format".into());
    }
    let screen = &setup.roots[screen];
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.visual_id == screen.root_visual)
        .ok_or("Cannot find the desktop visual")?;
    let masks = [visual.red_mask, visual.green_mask, visual.blue_mask];
    if masks.contains(&0) {
        return Err("The wallpaper requires a true-color visual".into());
    }
    let bytes_per_pixel = usize::from(format.bits_per_pixel / 8);
    let padding = usize::from(format.scanline_pad);
    let stride = (width * usize::from(format.bits_per_pixel)).div_ceil(padding) * padding / 8;
    if stride * height > 256 * 1024 * 1024 {
        return Err("The wallpaper exceeds the memory limit".into());
    }
    let reply = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            pixmap,
            0,
            0,
            geometry.width,
            geometry.height,
            u32::MAX,
        )?
        .reply()?;
    if reply.data.len() < stride * height {
        return Err("Incomplete wallpaper image".into());
    }
    let mut image = egui::ColorImage::new([width, height], egui::Color32::BLACK);
    for (row, pixels) in reply.data.chunks_exact(stride).take(height).enumerate() {
        for (column, bytes) in pixels[..width * bytes_per_pixel]
            .chunks_exact(bytes_per_pixel)
            .enumerate()
        {
            let mut raw = [0; 4];
            let pixel = if setup.image_byte_order == ImageOrder::LSB_FIRST {
                raw[..bytes_per_pixel].copy_from_slice(bytes);
                u32::from_le_bytes(raw)
            } else {
                raw[4 - bytes_per_pixel..].copy_from_slice(bytes);
                u32::from_be_bytes(raw)
            };
            let [red, green, blue] = masks.map(|mask| {
                let shift = mask.trailing_zeros();
                (u64::from((pixel & mask) >> shift) * 255 / u64::from(mask >> shift)) as u8
            });
            image.pixels[row * width + column] = egui::Color32::from_rgb(red, green, blue);
        }
    }
    Ok(image)
}
