use crate::{drag, places, FileManager};
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window};
use x11rb::rust_connection::RustConnection;

const TITLE: &str = "Still Files Desktop";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
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

pub struct Desktop {
    manager: FileManager,
    connection: RustConnection,
    atoms: Atoms,
    root: Window,
    geometry: [u32; 4],
    configured: bool,
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
            next_refresh: Instant::now(),
        })
    }

    fn refresh(&mut self) -> Result<()> {
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
            if let Err(error) = self.refresh() {
                eprintln!("Cannot update desktop window: {error}");
            }
            self.next_refresh = Instant::now() + Duration::from_secs(1);
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
                .with_transparent(true)
                .with_position([x, y])
                .with_inner_size([width, height]),
            |context, _| {
                if context.input(|input| input.viewport().close_requested()) {
                    context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                crate::apply_theme(context, self.manager.settings.dark_mode);
                eframe::App::update(&mut self.manager, context, frame);
            },
        );
    }
}
