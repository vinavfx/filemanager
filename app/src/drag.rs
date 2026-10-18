use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::render::ConnectionExt as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ColormapAlloc,
    ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, ImageFormat,
    ImageOrder, KeyButMask, PropMode, Rectangle, SelectionNotifyEvent, SelectionRequestEvent,
    StackMode, Visualid, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndProxy,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        XdndActionPrivate,
        TARGETS,
        UTF8_STRING,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DND,
        uri_list: b"text/uri-list",
        plain_text: b"text/plain",
    }
}

pub struct ExternalDrag {
    window: Option<Window>,
    cancelled: Arc<AtomicBool>,
}

impl ExternalDrag {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let window = context
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
                RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                _ => None,
            });
        Self {
            window,
            cancelled: Arc::default(),
        }
    }

    pub fn sibling(&self) -> Self {
        Self {
            window: self.window,
            cancelled: Arc::default(),
        }
    }

    pub fn start(&mut self, paths: Vec<PathBuf>) {
        self.cancel();
        let Some(window) = self.window else {
            return;
        };
        self.cancelled = Arc::default();
        let cancelled = Arc::clone(&self.cancelled);
        std::thread::spawn(move || {
            if let Err(error) =
                Session::new(window, paths).and_then(|session| session.run(&cancelled))
            {
                eprintln!("Cannot drag files to another application: {error}");
            }
        });
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for ExternalDrag {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Target {
    window: Window,
    proxy: Window,
    version: u32,
}

struct Session {
    connection: RustConnection,
    atoms: Atoms,
    root: Window,
    origin: Window,
    source: Window,
    uris: String,
    text: String,
    preview: bool,
    depth: u8,
    visual: Visualid,
    alpha_mask: u32,
}

impl Session {
    fn new(origin: Window, paths: Vec<PathBuf>) -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let mut origin = connection
            .get_input_focus()?
            .reply()
            .map_or(origin, |reply| match reply.focus {
                0 | 1 => origin,
                focus => focus,
            });
        while origin != root {
            let tree = connection.query_tree(origin)?.reply()?;
            if tree.parent == root || tree.parent == NONE {
                break;
            }
            origin = tree.parent;
        }
        let atoms = Atoms::new(&connection)?.reply()?;
        let source = connection.generate_id()?;
        let compositor = connection
            .intern_atom(false, format!("_NET_WM_CM_S{screen}").as_bytes())?
            .reply()?
            .atom;
        let composited = connection.get_selection_owner(compositor)?.reply()?.owner != NONE;
        let root_screen = &connection.setup().roots[screen];
        let mut depth = root_screen.root_depth;
        let mut visual = root_screen.root_visual;
        let mut alpha_mask = 0;
        let mut attributes = CreateWindowAux::new().override_redirect(1).border_pixel(0);
        if composited {
            let formats = connection.render_query_pict_formats()?.reply()?;
            let argb = formats.screens[screen]
                .depths
                .iter()
                .filter(|depth| depth.depth == 32)
                .flat_map(|depth| &depth.visuals)
                .find_map(|visual| {
                    formats
                        .formats
                        .iter()
                        .find(|format| format.id == visual.format && format.direct.alpha_mask != 0)
                        .map(|format| {
                            (
                                visual.visual,
                                u32::from(format.direct.alpha_mask) << format.direct.alpha_shift,
                            )
                        })
                });
            if let Some((argb_visual, mask)) = argb {
                depth = 32;
                visual = argb_visual;
                alpha_mask = mask;
                let colormap = connection.generate_id()?;
                connection
                    .create_colormap(ColormapAlloc::NONE, colormap, root, visual)?
                    .check()?;
                attributes = attributes.colormap(colormap).background_pixel(0);
            }
        }
        connection
            .create_window(
                depth,
                source,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &attributes,
            )?
            .check()?;
        connection
            .set_selection_owner(source, atoms.XdndSelection, CURRENT_TIME)?
            .check()?;
        let preview = paths.first().map(|path| {
            crate::images::drag_preview(path)
                .or_else(|| crate::icons::load(path))
                .unwrap_or_else(|| {
                    let folder = path.is_dir();
                    let mut image =
                        eframe::egui::ColorImage::new([64, 64], eframe::egui::Color32::TRANSPARENT);
                    for y in 8..56 {
                        for x in 8..56 {
                            let visible = if folder {
                                y >= 20 || (y >= 12 && x < 32)
                            } else {
                                x < 44 || y >= 20 || x - 44 <= y - 8
                            };
                            if visible {
                                image[(x, y)] = if folder {
                                    eframe::egui::Color32::from_rgb(112, 168, 157)
                                } else {
                                    eframe::egui::Color32::from_rgb(220, 224, 230)
                                };
                            }
                        }
                    }
                    image
                })
        });
        let mut uris = String::new();
        let mut quoted = Vec::new();
        for path in paths {
            let uri = url::Url::from_file_path(&path)
                .map_err(|()| format!("Invalid file path: {}", path.display()))?;
            uris.push_str(uri.as_str());
            uris.push_str("\r\n");
            quoted.push(format!(
                "'{}'",
                path.to_string_lossy().replace('\'', "'\\''")
            ));
        }
        connection.flush()?;
        let mut session = Self {
            connection,
            atoms,
            root,
            origin,
            source,
            uris,
            text: quoted.join(" "),
            preview: false,
            depth,
            visual,
            alpha_mask,
        };
        if let Some(image) = preview {
            match session.create_preview(&image, screen) {
                Ok(()) => session.preview = true,
                Err(error) => eprintln!("Cannot display the drag preview: {error}"),
            }
        }
        Ok(session)
    }

    fn create_preview(&self, image: &eframe::egui::ColorImage, screen: usize) -> Result<()> {
        let connection = &self.connection;
        connection.shape_query_version()?.reply()?;
        let screen = &connection.setup().roots[screen];
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == self.visual)
            .ok_or("Cannot find the root visual")?;
        let format = connection
            .setup()
            .pixmap_formats
            .iter()
            .find(|format| format.depth == self.depth)
            .ok_or("Cannot find the root pixel format")?;
        if !matches!(format.bits_per_pixel, 16 | 24 | 32)
            || [visual.red_mask, visual.green_mask, visual.blue_mask].contains(&0)
        {
            return Err("Unsupported drag preview pixel format".into());
        }
        let rgba: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(eframe::egui::Color32::to_srgba_unmultiplied)
            .collect();
        let image = image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, rgba)
            .ok_or("Invalid drag preview dimensions")?;
        let image = image::DynamicImage::ImageRgba8(image)
            .thumbnail(64, 64)
            .into_rgba8();
        let (width, height) = (image.width() as u16, image.height() as u16);
        let stride = (width as usize * format.bits_per_pixel as usize)
            .div_ceil(format.scanline_pad as usize)
            * (format.scanline_pad as usize / 8);
        let mut pixels = vec![0; stride * height as usize];
        let channel = |value: u8, mask: u32| {
            ((u32::from(value) * (mask >> mask.trailing_zeros()) / 255) << mask.trailing_zeros())
                & mask
        };
        let mut shape = Vec::new();
        for y in 0..height {
            let mut start = None;
            for x in 0..width {
                let pixel = image.get_pixel(x.into(), y.into()).0;
                let component = |value| {
                    if self.alpha_mask == 0 {
                        value
                    } else {
                        (u16::from(value) * u16::from(pixel[3]) / 255) as u8
                    }
                };
                let mut color = channel(component(pixel[0]), visual.red_mask)
                    | channel(component(pixel[1]), visual.green_mask)
                    | channel(component(pixel[2]), visual.blue_mask);
                if self.alpha_mask != 0 {
                    color |= channel(pixel[3], self.alpha_mask);
                }
                let bytes = if connection.setup().image_byte_order == ImageOrder::LSB_FIRST {
                    color.to_le_bytes()
                } else {
                    color.to_be_bytes()
                };
                let size = format.bits_per_pixel as usize / 8;
                let offset = y as usize * stride + x as usize * size;
                let bytes = if connection.setup().image_byte_order == ImageOrder::LSB_FIRST {
                    &bytes[..size]
                } else {
                    &bytes[4 - size..]
                };
                pixels[offset..offset + size].copy_from_slice(bytes);
                if pixel[3] >= 64 {
                    start.get_or_insert(x);
                }
                if (pixel[3] < 64 || x + 1 == width) && start.is_some() {
                    let left = start.take().unwrap_or(x);
                    let right = if pixel[3] < 64 { x } else { x + 1 };
                    shape.push(Rectangle {
                        x: left as i16,
                        y: y as i16,
                        width: right - left,
                        height: 1,
                    });
                }
            }
        }
        let pixmap = connection.generate_id()?;
        let gc = connection.generate_id()?;
        connection
            .create_pixmap(self.depth, pixmap, self.root, width, height)?
            .check()?;
        connection
            .create_gc(gc, pixmap, &CreateGCAux::new())?
            .check()?;
        connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                gc,
                width,
                height,
                0,
                0,
                0,
                self.depth,
                &pixels,
            )?
            .check()?;
        connection
            .change_window_attributes(
                self.source,
                &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
            )?
            .check()?;
        connection
            .configure_window(
                self.source,
                &ConfigureWindowAux::new()
                    .width(u32::from(width))
                    .height(u32::from(height)),
            )?
            .check()?;
        if self.alpha_mask == 0 {
            connection
                .shape_rectangles(
                    SO::SET,
                    SK::BOUNDING,
                    x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                    self.source,
                    0,
                    0,
                    &shape,
                )?
                .check()?;
        }
        connection
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                self.source,
                0,
                0,
                &[],
            )?
            .check()?;
        connection
            .change_property32(
                PropMode::REPLACE,
                self.source,
                self.atoms._NET_WM_WINDOW_TYPE,
                AtomEnum::ATOM,
                &[self.atoms._NET_WM_WINDOW_TYPE_DND],
            )?
            .check()?;
        connection.free_gc(gc)?.check()?;
        connection.free_pixmap(pixmap)?.check()?;
        Ok(())
    }

    fn property(&self, window: Window, property: Atom, kind: AtomEnum) -> Result<Option<u32>> {
        let reply = self
            .connection
            .get_property(false, window, property, kind, 0, 1)?
            .reply()?;
        Ok(reply.value32().and_then(|mut values| values.next()))
    }

    fn target(&self) -> Result<Option<Target>> {
        let mut window = self.root;
        let mut target = None;
        for _ in 0..32 {
            if window == self.origin {
                return Ok(None);
            }
            let proxy = self
                .property(window, self.atoms.XdndProxy, AtomEnum::WINDOW)?
                .filter(|proxy| {
                    self.property(*proxy, self.atoms.XdndProxy, AtomEnum::WINDOW)
                        .ok()
                        .flatten()
                        == Some(*proxy)
                })
                .unwrap_or(window);
            if let Some(version) = self.property(proxy, self.atoms.XdndAware, AtomEnum::ATOM)? {
                if version >= 3 {
                    target = Some(Target {
                        window,
                        proxy,
                        version: version.min(5),
                    });
                }
            }
            let child = self.connection.query_pointer(window)?.reply()?.child;
            if child == NONE {
                break;
            }
            window = child;
        }
        Ok(target)
    }

    fn send(&self, target: Target, message: Atom, data: [u32; 5]) -> Result<()> {
        let event = ClientMessageEvent::new(32, target.window, message, data);
        self.connection
            .send_event(false, target.proxy, EventMask::NO_EVENT, event)?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }

    fn selection(&self, request: SelectionRequestEvent) -> Result<()> {
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let supported = [
            self.atoms.uri_list,
            self.atoms.plain_text,
            self.atoms.UTF8_STRING,
        ];
        let mut response = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: NONE,
        };
        if request.selection == self.atoms.XdndSelection {
            if request.target == self.atoms.TARGETS {
                let targets = [supported[0], supported[1], supported[2], self.atoms.TARGETS];
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        AtomEnum::ATOM,
                        &targets,
                    )?
                    .check()?;
                response.property = property;
            } else if supported.contains(&request.target) {
                let data = if request.target == self.atoms.uri_list {
                    &self.uris
                } else {
                    &self.text
                };
                if data.len() + 64 <= self.connection.maximum_request_bytes() {
                    self.connection
                        .change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            property,
                            request.target,
                            data.as_bytes(),
                        )?
                        .check()?;
                    response.property = property;
                }
            }
        }
        self.connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, response)?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }

    fn run(self, cancelled: &AtomicBool) -> Result<()> {
        let mut target: Option<Target> = None;
        let mut accepted = false;
        let mut pending = false;
        let mut position = None;
        let mut released = None;
        let mut dropped = None;
        let mut preview_position = None;
        loop {
            while let Some(event) = self.connection.poll_for_event()? {
                match event {
                    Event::SelectionRequest(request) => self.selection(request)?,
                    Event::SelectionClear(_) => return Ok(()),
                    Event::ClientMessage(event) => {
                        let data = event.data.as_data32();
                        if target.is_some_and(|target| target.window == data[0]) {
                            if event.type_ == self.atoms.XdndStatus {
                                pending = false;
                                accepted = data[1] & 1 != 0
                                    && (data[4] == self.atoms.XdndActionCopy
                                        || data[4] == self.atoms.XdndActionPrivate);
                            } else if event.type_ == self.atoms.XdndFinished && dropped.is_some() {
                                return Ok(());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if dropped.is_some_and(|time: Instant| time.elapsed() > Duration::from_secs(10)) {
                return Ok(());
            }
            if dropped.is_none() {
                if cancelled.load(Ordering::Relaxed) {
                    if let Some(target) = target {
                        self.send(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                    }
                    return Ok(());
                }
                let pointer = self.connection.query_pointer(self.root)?.reply()?;
                if self.preview {
                    if pointer.mask.contains(KeyButMask::BUTTON1) {
                        let coordinates = (pointer.root_x, pointer.root_y);
                        if preview_position != Some(coordinates) {
                            self.connection.configure_window(
                                self.source,
                                &ConfigureWindowAux::new()
                                    .x(i32::from(pointer.root_x) + 16)
                                    .y(i32::from(pointer.root_y) + 16)
                                    .stack_mode(StackMode::ABOVE),
                            )?;
                            if preview_position.is_none() {
                                self.connection.map_window(self.source)?;
                            }
                            self.connection.clear_area(false, self.source, 0, 0, 0, 0)?;
                            preview_position = Some(coordinates);
                            self.connection.flush()?;
                        }
                    } else {
                        self.connection.unmap_window(self.source)?;
                        self.connection.flush()?;
                    }
                }
                let current = self.target()?;
                if current != target {
                    if let Some(target) = target {
                        self.send(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                    }
                    target = current;
                    accepted = false;
                    pending = false;
                    position = None;
                    if let Some(target) = target {
                        self.send(
                            target,
                            self.atoms.XdndEnter,
                            [
                                self.source,
                                target.version << 24,
                                self.atoms.uri_list,
                                self.atoms.plain_text,
                                self.atoms.UTF8_STRING,
                            ],
                        )?;
                    }
                }
                let coordinates =
                    ((pointer.root_x as u16 as u32) << 16) | pointer.root_y as u16 as u32;
                if !pointer.mask.contains(KeyButMask::BUTTON1) {
                    let time = *released.get_or_insert_with(Instant::now);
                    if let Some(target) = target {
                        if pending && time.elapsed() < Duration::from_millis(500) {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        if accepted && !pending {
                            self.send(
                                target,
                                self.atoms.XdndDrop,
                                [self.source, 0, CURRENT_TIME, 0, 0],
                            )?;
                            dropped = Some(Instant::now());
                        } else {
                            self.send(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                            return Ok(());
                        }
                    } else {
                        return Ok(());
                    }
                } else if !pending && position != Some(coordinates) {
                    if let Some(target) = target {
                        self.send(
                            target,
                            self.atoms.XdndPosition,
                            [
                                self.source,
                                0,
                                coordinates,
                                CURRENT_TIME,
                                self.atoms.XdndActionCopy,
                            ],
                        )?;
                        pending = true;
                        position = Some(coordinates);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
