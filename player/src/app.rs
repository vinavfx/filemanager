use crate::media::{Event, Frame, Loader, Media, Request};
use crate::playback::{Direction, Playback, Rate};
use crate::renderer::Renderer;
use eframe::egui::{
    self, Align2, Color32, FontId, Key, PointerButton, Pos2, Rect, Sense, Stroke, Vec2,
    ViewportCommand,
};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub struct Player {
    pub quit_requested: bool,
    pub hide_requested: bool,
    loader: Loader,
    renderer: Arc<Mutex<Renderer>>,
    media: Option<Media>,
    playback: Playback,
    generation: u64,
    requested: Option<usize>,
    loading: bool,
    error: Option<String>,
    path: Option<PathBuf>,
    picker: Option<mpsc::Receiver<Result<Option<PathBuf>, String>>>,
    frame_text: String,
    rate_text: String,
    zoom_text: String,
    zoom: f32,
    rotation: i32,
    origin: Vec2,
    fit: bool,
    auto_fit_window: bool,
    viewer: Rect,
    fullscreen: bool,
    fit_fill: bool,
    show_ui: bool,
    fullscreen_previous_ui: bool,
    hud: bool,
    channel: i32,
    stored: Option<Arc<Frame>>,
    preview: Option<Arc<Frame>>,
    preview_size: Option<Vec2>,
    displayed_frame: Option<(Arc<Frame>, Vec2, Vec2)>,
    show_stored: bool,
    drag_origin: Vec2,
    pan_origin: Vec2,
    drag_frame: usize,
    drag_zoom: f32,
    drag_mode: u8,
    timeline_dragging: bool,
    timeline_rect: Rect,
    measured_start: Instant,
    measured_frames: usize,
    actual_rate: f64,
}

impl Player {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
        still: bool,
    ) -> Result<Self, String> {
        let gl = context.gl.as_ref().ok_or("OpenGL is unavailable")?;
        let renderer = Arc::new(Mutex::new(Renderer::new(gl)?));
        configure_style(&context.egui_ctx);
        let mut player = Self {
            quit_requested: false,
            hide_requested: false,
            loader: Loader::new(context.egui_ctx.clone()),
            renderer,
            media: None,
            playback: Playback::default(),
            generation: 0,
            requested: None,
            loading: false,
            error: None,
            path: None,
            picker: None,
            frame_text: "0".into(),
            rate_text: "24".into(),
            zoom_text: "1".into(),
            zoom: 1.0,
            rotation: 0,
            origin: Vec2::ZERO,
            fit: true,
            auto_fit_window: true,
            viewer: Rect::NOTHING,
            fullscreen: false,
            fit_fill: false,
            show_ui: true,
            fullscreen_previous_ui: true,
            hud: false,
            channel: 0,
            stored: None,
            preview: None,
            preview_size: None,
            displayed_frame: None,
            show_stored: false,
            drag_origin: Vec2::ZERO,
            pan_origin: Vec2::ZERO,
            drag_frame: 0,
            drag_zoom: 1.0,
            drag_mode: 0,
            timeline_dragging: false,
            timeline_rect: Rect::NOTHING,
            measured_start: Instant::now(),
            measured_frames: 0,
            actual_rate: 0.0,
        };
        if let Some(path) = path {
            player.open_with_mode(path, still);
        }
        Ok(player)
    }

    pub fn open(&mut self, path: PathBuf) {
        self.open_with_mode(path, false);
    }

    pub fn open_with_mode(&mut self, path: PathBuf, still: bool) {
        if let Ok(mut renderer) = self.renderer.lock() {
            renderer.active = true;
        }
        self.generation += 1;
        self.path = Some(path.clone());
        self.preview = None;
        self.preview_size = None;
        self.media = None;
        self.stored = None;
        self.show_stored = false;
        self.fit = true;
        self.playback = Playback::default();
        self.loading = true;
        self.error = None;
        self.requested = None;
        self.loader
            .send(Request::Open(self.generation, path, still));
    }

    pub fn close(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Ok(mut renderer) = self.renderer.lock() {
            renderer.clear(gl);
        }
        self.picker = None;
        self.generation += 1;
        self.loader.send(Request::Close(self.generation));
        self.media = None;
        self.preview = None;
        self.preview_size = None;
        self.displayed_frame = None;
        self.auto_fit_window = true;
        self.path = None;
        self.loading = false;
        self.error = None;
        self.stored = None;
        self.show_stored = false;
        self.playback = Playback::default();
    }

    fn pick_file(&mut self) {
        if self.picker.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.picker = Some(receiver);
        std::thread::spawn(move || {
            let result = Command::new("zenity").args(["--file-selection", "--title=Open media"])
                .stdin(Stdio::null()).stderr(Stdio::null()).output()
                .map_err(|error| format!("Cannot open file chooser: {error}. Drop a file or pass its path on the command line."))
                .and_then(|output| {
                    if output.status.success() {
                        use std::os::unix::ffi::OsStringExt;
                        let mut bytes = output.stdout;
                        if bytes.last() == Some(&b'\n') { bytes.pop(); }
                        Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(bytes))))
                    } else if output.status.code() == Some(1) {
                        Ok(None)
                    } else {
                        Err("File chooser failed. Drop a file or pass its path on the command line.".into())
                    }
                });
            let _ = sender.send(result);
        });
    }

    pub fn poll(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.picker
            && let Ok(result) = receiver.try_recv()
        {
            self.picker = None;
            match result {
                Ok(Some(path)) => self.open(path),
                Err(error) => self.error = Some(error),
                _ => {}
            }
        }
        while let Ok(event) = self.loader.events.try_recv() {
            match event {
                Event::Preview(id, frame, (width, height)) if id == self.generation => {
                    self.preview = Some(frame);
                    self.preview_size = Some(egui::vec2(width as f32, height as f32));
                    self.fit_view();
                }
                Event::Loaded(id, media) if id == self.generation => {
                    self.playback.load(media.count, media.rate);
                    self.rate_text = media.rate.label();
                    self.frame_text = media.number(0).to_string();
                    context.send_viewport_cmd(ViewportCommand::Title(format!(
                        "{} - Player",
                        media.path.file_name().unwrap_or_default().to_string_lossy()
                    )));
                    let previous_size = self.image_size();
                    self.media = Some(media);
                    self.loading = false;
                    if self.preview.is_some() {
                        self.zoom *= previous_size.x / self.image_size().x;
                    } else {
                        self.fit = true;
                    }
                    self.measured_start = Instant::now();
                    self.measured_frames = 0;
                }
                Event::Error(id, error) if id == self.generation => {
                    self.error = Some(error);
                    self.loading = false;
                    self.playback.set_direction(Direction::Stop);
                }
                _ => {}
            }
        }
        if let Some(error) = self
            .renderer
            .lock()
            .ok()
            .and_then(|renderer| renderer.error.clone())
        {
            self.error = Some(error);
        }
    }

    fn request(&mut self, frame: usize) {
        if self.requested != Some(frame) {
            self.loader.send(Request::Seek(self.generation, frame));
            self.requested = Some(frame);
        }
    }

    fn tick(&mut self) {
        if self.timeline_dragging || !self.playback.due() {
            return;
        }
        let next = self.playback.next_frame();
        let frame = self.playback.mapped(next, true);
        if self
            .loader
            .frame(self.generation, self.playback.frame)
            .is_none()
        {
            return;
        }
        if self.loader.frame(self.generation, frame).is_some() {
            self.playback.seek(next, true);
            self.playback.consume_tick();
            self.measured_frames += 1;
        } else {
            self.request(frame);
        }
        let elapsed = self.measured_start.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            self.actual_rate = self.measured_frames as f64 / elapsed;
            self.measured_start = Instant::now();
            self.measured_frames = 0;
        }
    }

    fn playback_toolbar(&mut self, context: &egui::Context) {
        if self.playback.count < 2 {
            return;
        }
        egui::TopBottomPanel::bottom("playback_toolbar")
            .exact_height(36.0)
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.button("⏮").on_hover_text("Go to start (_)").clicked() {
                        self.playback.seek(0, false);
                    }
                    for (label, direction, hint) in [
                        ("◀", Direction::Reverse, "Reverse (J)"),
                        ("■", Direction::Stop, "Stop (K)"),
                        ("▶", Direction::Forward, "Forward (L)"),
                    ] {
                        if ui
                            .selectable_label(self.playback.direction == direction, label)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            self.playback.set_direction(direction);
                        }
                    }
                    if ui
                        .button("⏭")
                        .on_hover_text("Go to end (Shift+4)")
                        .clicked()
                    {
                        self.playback
                            .seek(self.playback.count.saturating_sub(1) as i64, false);
                    }
                    ui.separator();
                    if ui
                        .button(self.playback.loop_mode.label())
                        .on_hover_text("Cycle loop mode (;) ")
                        .clicked()
                    {
                        self.playback.set_loop(self.playback.loop_mode.next());
                    }
                    ui.separator();
                    ui.label("FPS");
                    let response =
                        ui.add(egui::TextEdit::singleline(&mut self.rate_text).desired_width(78.0));
                    if response.lost_focus() {
                        self.apply_rate();
                    }
                    response.on_hover_text(format!(
                        "{}\nOr enter an exact fraction, such as 24000/1001",
                        Rate::PRESETS.join(", ")
                    ));
                    ui.label("Frame");
                    let frame_id = ui.make_persistent_id("frame_input");
                    if !ui.memory(|memory| {
                        memory.has_focus(frame_id) || memory.had_focus_last_frame(frame_id)
                    }) {
                        self.frame_text = self
                            .media
                            .as_ref()
                            .map(|media| media.number(self.playback.frame))
                            .unwrap_or(0)
                            .to_string();
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.frame_text)
                            .id(frame_id)
                            .desired_width(78.0),
                    );
                    if response.lost_focus()
                        && let Ok(number) = self.frame_text.trim().parse::<i64>()
                        && let Some(media) = &self.media
                    {
                        let frame = media.index(number);
                        self.playback.set_direction(Direction::Stop);
                        self.playback.seek(frame as i64, false);
                    }
                });
            });
    }

    fn toolbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(36.0)
            .show(context, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label("Zoom");
                    let zoom_id = ui.make_persistent_id("zoom_input");
                    if !ui.memory(|memory| {
                        memory.has_focus(zoom_id) || memory.had_focus_last_frame(zoom_id)
                    }) {
                        self.zoom_text = format!("{:.3}", self.zoom);
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.zoom_text)
                            .id(zoom_id)
                            .desired_width(60.0),
                    );
                    if response.lost_focus() {
                        let value = if let Some(percent) = self.zoom_text.trim().strip_suffix('%') {
                            percent.parse::<f32>().ok().map(|value| value / 100.0)
                        } else {
                            self.zoom_text.trim().parse().ok()
                        };
                        if let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0)
                        {
                            self.set_zoom(value, self.viewer.center());
                        }
                    }
                    if ui
                        .selectable_label(self.fit && self.fit_fill, "Fit")
                        .on_hover_text("Fill the viewer without black bars (Ctrl+F)")
                        .clicked()
                    {
                        self.fit_fill = !(self.fit && self.fit_fill);
                        self.fit_view();
                    }
                    ui.separator();
                    for (rotation, label) in [(-90, "−90°"), (0, "0°"), (90, "+90°")] {
                        if ui
                            .selectable_label(self.rotation == rotation, label)
                            .clicked()
                        {
                            self.set_rotation(rotation);
                        }
                    }
                    ui.separator();
                    for (channel, label) in [(0, "RGB"), (1, "R"), (2, "G"), (3, "B"), (4, "A")] {
                        if ui
                            .selectable_label(self.channel == channel, label)
                            .clicked()
                        {
                            self.channel = if self.channel == channel { 0 } else { channel };
                        }
                    }
                });
            });
    }

    fn apply_rate(&mut self) {
        if let Some(rate) = Rate::parse(&self.rate_text) {
            self.playback.rate = rate;
            self.playback.reset_clock();
        }
        self.rate_text = self.playback.rate.label();
    }

    fn timeline(&mut self, context: &egui::Context) {
        if self.playback.count < 2 {
            self.timeline_rect = Rect::NOTHING;
            return;
        }
        egui::TopBottomPanel::bottom("timeline")
            .exact_height(48.0)
            .show(context, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                self.timeline_rect = rect;
                let count = self.playback.count;
                if count == 0 {
                    return;
                }
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, Color32::from_gray(38));
                let frame_width = rect.width() / count as f32;
                let x = |index: usize| rect.left() + index as f32 * frame_width;
                let font = FontId::proportional(12.0);
                let second = self.playback.rate.value().floor().max(1.0) as usize;
                let step = if frame_width > 3.0 {
                    1
                } else {
                    second.max((3.0 / frame_width).ceil() as usize)
                };
                let mut label_right = f32::NEG_INFINITY;
                for index in (0..count).step_by(step) {
                    let px = x(index) + frame_width / 2.0;
                    let is_second = index % second == 0;
                    let top = if is_second {
                        rect.center().y - 4.0
                    } else {
                        rect.bottom() - 8.0
                    };
                    painter.line_segment(
                        [Pos2::new(px, top), Pos2::new(px, rect.bottom() - 3.0)],
                        Stroke::new(1.0, Color32::from_gray(110)),
                    );
                    if is_second && px >= label_right {
                        let text = self
                            .media
                            .as_ref()
                            .map(|media| media.number(index))
                            .unwrap_or(index as i64)
                            .to_string();
                        let galley =
                            painter.layout_no_wrap(text, font.clone(), Color32::from_gray(190));
                        label_right = px + galley.size().x + 14.0;
                        painter.galley(
                            Pos2::new(px + 5.0, rect.center().y),
                            galley,
                            Color32::from_gray(190),
                        );
                    }
                }
                if self.playback.in_point != 0 || self.playback.out_point != count - 1 {
                    let start = self.playback.in_point.min(self.playback.out_point);
                    let end = self.playback.in_point.max(self.playback.out_point);
                    let color = Color32::from_rgb(178, 153, 76);
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x(start), rect.top()),
                            Pos2::new(x(end + 1), rect.top() + 2.0),
                        ),
                        0.0,
                        color,
                    );
                }
                if let Ok(cache) = self.loader.cache.lock()
                    && cache.generation == self.generation
                {
                    let mut range: Option<(usize, usize)> = None;
                    let paint_range = |start: usize, end: usize| {
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(x(start), rect.bottom() - 2.0),
                                Pos2::new(x(end + 1), rect.bottom()),
                            ),
                            0.0,
                            Color32::from_rgb(90, 90, 255),
                        );
                    };
                    for &index in cache.frames.keys() {
                        match range {
                            Some((start, end)) if index == end + 1 => range = Some((start, index)),
                            Some((start, end)) => {
                                paint_range(start, end);
                                range = Some((index, index));
                            }
                            None => range = Some((index, index)),
                        }
                    }
                    if let Some((start, end)) = range {
                        paint_range(start, end);
                    }
                }
                let current_x = x(self.playback.frame) + frame_width / 2.0;
                painter.line_segment(
                    [
                        Pos2::new(current_x, rect.top()),
                        Pos2::new(current_x, rect.bottom()),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(178, 153, 76)),
                );
                let text = self
                    .media
                    .as_ref()
                    .map(|media| media.number(self.playback.frame))
                    .unwrap_or(0)
                    .to_string();
                let align = if current_x > rect.right() - 60.0 {
                    Align2::RIGHT_TOP
                } else {
                    Align2::LEFT_TOP
                };
                let offset = if align == Align2::RIGHT_TOP {
                    -5.0
                } else {
                    5.0
                };
                painter.text(
                    Pos2::new(current_x + offset, rect.top() + 4.0),
                    align,
                    text,
                    font,
                    Color32::WHITE,
                );
            });
    }

    fn timeline_mouse(&mut self, context: &egui::Context) {
        if !self.show_ui || self.playback.count < 2 {
            self.timeline_dragging = false;
            return;
        }
        let (position, pressed, down) = context.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.any_pressed(),
                input.pointer.any_down(),
            )
        });
        if pressed && position.is_some_and(|position| self.timeline_rect.contains(position)) {
            self.timeline_dragging = true;
        }
        if self.timeline_dragging {
            if let Some(position) = position {
                let frame_width = self.timeline_rect.width() / self.playback.count as f32;
                let frame = ((position.x - self.timeline_rect.left()) / frame_width)
                    .floor()
                    .clamp(0.0, (self.playback.count - 1) as f32)
                    as i64;
                self.playback.seek(frame, false);
            }
            self.playback.reset_clock();
            self.timeline_dragging = down;
        }
    }

    fn image_size(&self) -> Vec2 {
        let size = self
            .media
            .as_ref()
            .map(|media| egui::vec2(media.width as f32, media.height as f32))
            .or(self.preview_size)
            .unwrap_or(Vec2::splat(1.0));
        if self.rotation == 0 {
            size
        } else {
            egui::vec2(size.y, size.x)
        }
    }

    fn set_rotation(&mut self, rotation: i32) {
        if self.rotation == rotation {
            return;
        }
        let previous_size = self.image_size();
        self.rotation = rotation;
        if self.media.is_none() {
            return;
        }
        if self.fit {
            self.fit_view();
        } else {
            self.origin += (previous_size - self.image_size()) * self.zoom / 2.0;
        }
    }

    fn fit_view(&mut self) {
        let size = self.image_size();
        let width_ratio = self.viewer.width() / size.x;
        let height_ratio = self.viewer.height() / size.y;
        self.zoom = if self.fit_fill {
            width_ratio.max(height_ratio)
        } else {
            width_ratio.min(height_ratio)
        }
        .max(0.0001);
        self.origin = (self.viewer.size() - size * self.zoom) / 2.0;
        self.fit = true;
    }

    fn set_zoom(&mut self, zoom: f32, focus: Pos2) {
        let zoom = zoom.clamp(0.0001, 1024.0);
        let focus = focus - self.viewer.min;
        self.origin = focus + (self.origin - focus) * (zoom / self.zoom);
        self.zoom = zoom;
        self.fit = false;
    }

    fn focus(&self, context: &egui::Context) -> Pos2 {
        context
            .input(|input| input.pointer.hover_pos())
            .filter(|position| self.viewer.contains(*position))
            .unwrap_or(self.viewer.center())
    }

    fn fit_window(&mut self, context: &egui::Context) {
        let screen = context
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(egui::vec2(1280.0, 800.0));
        let maximum = screen * 0.75;
        let ratio = (maximum.x / 1280.0).min(maximum.y / 720.0);
        let target = egui::vec2(1280.0, 720.0) * ratio;
        let decorations = context.input(|input| {
            let viewport = input.viewport();
            match (viewport.outer_rect, viewport.inner_rect) {
                (Some(outer), Some(inner)) => outer.size() - inner.size(),
                _ => Vec2::ZERO,
            }
        });
        if self.fullscreen {
            self.fullscreen(context);
        }
        context.send_viewport_cmd(ViewportCommand::Maximized(false));
        context.send_viewport_cmd(ViewportCommand::InnerSize(target));
        context.send_viewport_cmd(ViewportCommand::OuterPosition(
            ((screen - target - decorations) / 2.0).to_pos2(),
        ));
        self.fit = true;
    }

    fn viewer(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(context, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                let previous_size = self.viewer.size();
                self.viewer = rect;
                if self.fit {
                    self.fit_view();
                } else if previous_size != rect.size()
                    && previous_size.is_finite()
                    && previous_size.min_elem() > 0.0
                    && rect.size().min_elem() > 0.0
                {
                    let image_size = self.image_size();
                    let previous_scale = (previous_size / image_size).min_elem();
                    let scale = (rect.size() / image_size).min_elem() / previous_scale;
                    self.origin = rect.size() / 2.0 + (self.origin - previous_size / 2.0) * scale;
                    self.zoom *= scale;
                }
                if self.auto_fit_window {
                    self.fit_window(context);
                    self.fit_view();
                    self.auto_fit_window = false;
                }
                self.viewer_mouse(context, &response);
                let frame = if self.show_stored {
                    self.stored.clone()
                } else {
                    self.loader.frame(self.generation, self.playback.frame)
                };
                let image_ready = frame.is_some();
                if image_ready && !self.show_stored {
                    self.preview = None;
                }
                let image_size = self.image_size() * self.zoom;
                if let Some(frame) = frame.or_else(|| self.preview.clone()) {
                    self.displayed_frame = Some((frame, self.origin, image_size));
                }
                if let Some((frame, origin, size)) = &self.displayed_frame {
                    ui.painter().add(Renderer::paint(
                        self.renderer.clone(),
                        frame.clone(),
                        rect,
                        *origin,
                        *size,
                        self.channel,
                        self.rotation,
                    ));
                }
                if !image_ready && (self.loading || self.media.is_some()) && self.error.is_none() {
                    let track =
                        Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 1.0), rect.max);
                    let phase = context.input(|input| input.time) as f32;
                    let width = track.width() * 0.25;
                    let offset = (phase * 3.0).sin() * 0.5 + 0.5;
                    let bar = Rect::from_min_size(
                        track.min + egui::vec2((track.width() - width) * offset, 0.0),
                        egui::vec2(width, track.height()),
                    );
                    let painter = context
                        .layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("loading_overlay"),
                        ))
                        .with_clip_rect(rect);
                    painter.rect_filled(track, 0.0, Color32::from_gray(38));
                    painter.rect_filled(bar, 0.0, Color32::from_rgb(90, 90, 255));
                    context.request_repaint_after(Duration::from_millis(16));
                } else if self.media.is_none() && self.preview.is_none() && !self.loading {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "Drop a file or press Ctrl+O",
                        FontId::proportional(14.0),
                        Color32::GRAY,
                    );
                }
                if self.hud
                    && image_ready
                    && let Some(media) = &self.media
                {
                    let painter = ui.painter().with_clip_rect(rect);
                    let width = (rect.width() - 64.0).clamp(1.0, 340.0);
                    let title = painter.layout(
                        media
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        FontId::proportional(17.0),
                        Color32::from_rgb(235, 240, 248),
                        width,
                    );
                    let size = egui::vec2(width + 32.0, title.size().y + 144.0);
                    let position = rect.max - Vec2::splat(16.0) - size;
                    let content = position + Vec2::splat(16.0);
                    let panel = Rect::from_min_size(position, size);
                    painter.rect_filled(
                        panel,
                        10.0,
                        Color32::from_rgba_unmultiplied(22, 26, 33, 225),
                    );
                    painter.rect_stroke(
                        panel,
                        10.0,
                        Stroke::new(1.0, Color32::from_white_alpha(35)),
                        egui::StrokeKind::Inside,
                    );
                    let rows_y = content.y + title.size().y + 22.0;
                    painter.galley(content, title, Color32::WHITE);
                    painter.line_segment(
                        [
                            egui::pos2(content.x, rows_y - 10.0),
                            egui::pos2(panel.right() - 16.0, rows_y - 10.0),
                        ],
                        Stroke::new(1.0, Color32::from_white_alpha(25)),
                    );
                    for (index, (label, value)) in [
                        ("Resolution", format!("{} × {}", media.width, media.height)),
                        ("Frame", media.number(self.playback.frame).to_string()),
                        ("Target FPS", format!("{:.2}", self.playback.rate.value())),
                        ("Actual FPS", format!("{:.2}", self.actual_rate)),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let row = egui::pos2(content.x, rows_y + index as f32 * 25.0);
                        painter.text(
                            row,
                            Align2::LEFT_TOP,
                            label,
                            FontId::proportional(15.0),
                            Color32::from_rgb(155, 167, 185),
                        );
                        painter.text(
                            egui::pos2(panel.right() - 16.0, row.y),
                            Align2::RIGHT_TOP,
                            value,
                            FontId::monospace(15.0),
                            Color32::from_rgb(205, 225, 250),
                        );
                    }
                }
                if let Some(error) = &self.error {
                    let error_rect = Rect::from_min_size(
                        rect.min + Vec2::splat(12.0),
                        egui::vec2((rect.width() - 24.0).max(1.0), 75.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(error_rect), |ui| {
                        egui::Frame::new()
                            .fill(Color32::from_black_alpha(230))
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.colored_label(Color32::from_rgb(255, 160, 140), error);
                                if ui.small_button("Dismiss").clicked() {
                                    context.data_mut(|data| {
                                        data.insert_temp(egui::Id::new("dismiss_error"), true)
                                    });
                                }
                            });
                    });
                }
            });
        if context
            .data_mut(|data| data.remove_temp::<bool>(egui::Id::new("dismiss_error")))
            .unwrap_or(false)
        {
            self.error = None;
        }
    }

    fn viewer_mouse(&mut self, context: &egui::Context, response: &egui::Response) {
        let (position, modifiers, pressed, down, events) = context.input(|input| {
            (
                input.pointer.interact_pos(),
                input.modifiers,
                input.pointer.any_pressed(),
                input.pointer.any_down(),
                input.events.clone(),
            )
        });
        if let Some(position) = position.filter(|position| self.viewer.contains(*position))
            && pressed
        {
            let (left, middle, right) = context.input(|input| {
                (
                    input.pointer.button_pressed(PointerButton::Primary),
                    input.pointer.button_pressed(PointerButton::Middle),
                    input.pointer.button_pressed(PointerButton::Secondary),
                )
            });
            if modifiers.ctrl && (middle || right) {
                self.set_zoom(self.zoom * if middle { 2.0 } else { 0.5 }, position);
            } else if middle && modifiers.alt {
                self.drag_mode = 3;
                self.drag_zoom = self.zoom;
            } else if (middle && !modifiers.ctrl) || (left && (modifiers.shift || modifiers.alt)) {
                self.drag_mode = 1;
                self.pan_origin = self.origin;
            } else if left && modifiers.ctrl {
                self.drag_mode = 2;
                self.drag_frame = self.playback.frame;
                self.playback.set_direction(Direction::Stop);
            }
            if self.drag_mode != 0 {
                self.drag_origin = position.to_vec2();
                context.memory_mut(|memory| {
                    memory.surrender_focus(memory.focused().unwrap_or(egui::Id::NULL))
                });
            }
        }
        if down {
            if self.drag_mode == 1 {
                if let Some(position) = position {
                    self.origin = self.pan_origin + position.to_vec2() - self.drag_origin;
                }
                self.fit = false;
                context.set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if self.drag_mode == 2
                && let Some(position) = position
            {
                let offset = ((position.x - self.drag_origin.x) / 5.0).trunc() as i64;
                self.playback.seek(self.drag_frame as i64 + offset, false);
                context.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if self.drag_mode == 3
                && let Some(position) = position
            {
                let offset = ((position.x - self.drag_origin.x) / 200.0).clamp(-32.0, 32.0);
                self.set_zoom(
                    self.drag_zoom * 2.0_f32.powf(offset),
                    self.drag_origin.to_pos2(),
                );
                context.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
        } else {
            self.drag_mode = 0;
        }
        if !response.hovered() {
            return;
        }
        for event in events {
            if let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } = event
            {
                let amount = match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / 40.0,
                    egui::MouseWheelUnit::Page => delta.y,
                };
                if amount == 0.0 {
                    continue;
                }
                if modifiers.shift {
                    self.playback.set_direction(Direction::Stop);
                    self.playback
                        .seek(self.playback.frame as i64 + amount.round() as i64, false);
                } else if modifiers.ctrl {
                    let value = (self.playback.rate.value() + amount as f64)
                        .round()
                        .clamp(1.0, 1000.0);
                    if let Some(rate) = Rate::parse(&format!("{value:.0}")) {
                        self.playback.rate = rate;
                        self.rate_text = rate.label();
                        self.playback.reset_clock();
                    }
                } else {
                    self.set_zoom(
                        self.zoom * if amount > 0.0 { 1.1 } else { 1.0 / 1.1 },
                        self.focus(context),
                    );
                }
            }
        }
    }

    fn fullscreen(&mut self, context: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        if self.fullscreen {
            self.fullscreen_previous_ui = self.show_ui;
            self.show_ui = false;
        } else {
            self.show_ui = self.fullscreen_previous_ui;
        }
        self.fit = false;
        context.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn shortcuts(&mut self, context: &egui::Context, gl: Option<&eframe::glow::Context>) {
        if context.wants_keyboard_input() {
            return;
        }
        let events = context.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Text(text) = &event
                && text == "_"
                && context.input(|input| {
                    !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.mac_cmd
                })
            {
                self.playback.seek(0, false);
                context.request_repaint();
                continue;
            }
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                repeat,
                ..
            } = event
            else {
                continue;
            };
            if modifiers.alt || modifiers.mac_cmd {
                continue;
            }
            context.request_repaint();
            if modifiers.ctrl {
                match key {
                    Key::O if !modifiers.shift => self.pick_file(),
                    Key::Q if !modifiers.shift => self.quit_requested = true,
                    Key::E if !modifiers.shift => self.hide_requested = true,
                    Key::W if !modifiers.shift => self.close(gl),
                    Key::H if !modifiers.shift => self.hud = !self.hud,
                    Key::F if !modifiers.shift => {
                        self.fit_fill = true;
                        self.fit_view();
                    }
                    Key::R if modifiers.shift => {
                        self.loader
                            .send(Request::ReloadFrame(self.generation, self.playback.frame));
                        self.requested = None;
                        self.error = None;
                    }
                    Key::R => {
                        if let Some(path) = self.path.clone() {
                            self.open(path);
                        }
                    }
                    Key::OpenBracket if !modifiers.shift => {
                        self.playback.seek(self.playback.frame as i64 - 100, false)
                    }
                    Key::CloseBracket if !modifiers.shift => {
                        self.playback.seek(self.playback.frame as i64 + 100, false)
                    }
                    _ => {}
                }
                continue;
            }
            if modifiers.shift
                && !matches!(
                    key,
                    Key::H
                        | Key::L
                        | Key::I
                        | Key::O
                        | Key::E
                        | Key::OpenBracket
                        | Key::CloseBracket
                        | Key::OpenCurlyBracket
                        | Key::CloseCurlyBracket
                        | Key::Minus
                        | Key::Num4
                )
            {
                continue;
            }
            match key {
                Key::J | Key::K if !modifiers.shift && !repeat => {
                    let direction = if key == Key::J {
                        Direction::Reverse
                    } else {
                        Direction::Forward
                    };
                    self.playback
                        .set_direction(if self.playback.direction == direction {
                            Direction::Stop
                        } else {
                            direction
                        });
                }
                Key::H | Key::L
                    if !modifiers.shift && self.media.as_ref().is_some_and(Media::is_still) =>
                {
                    if let Some(path) = self.path.clone() {
                        crate::instance::navigate_still(path, key == Key::L);
                    }
                }
                Key::H if modifiers.shift => {
                    self.playback.seek(self.playback.frame as i64 - 10, false);
                }
                Key::L if modifiers.shift => {
                    self.playback.seek(self.playback.frame as i64 + 10, false);
                }
                Key::ArrowLeft | Key::H if !modifiers.shift => {
                    self.playback.set_direction(Direction::Stop);
                    self.playback.seek(self.playback.frame as i64 - 1, false);
                }
                Key::ArrowRight | Key::L if !modifiers.shift => {
                    self.playback.set_direction(Direction::Stop);
                    self.playback.seek(self.playback.frame as i64 + 1, false);
                }
                Key::Space if !modifiers.shift => self.playback.toggle(),
                Key::Minus if modifiers.shift => self.playback.seek(0, false),
                Key::Num4 if modifiers.shift => self
                    .playback
                    .seek(self.playback.count.saturating_sub(1) as i64, false),
                Key::Home => self.playback.seek(self.playback.in_point as i64, true),
                Key::End => self.playback.seek(self.playback.out_point as i64, true),
                Key::OpenBracket | Key::OpenCurlyBracket => {
                    let amount = if modifiers.shift || key == Key::OpenCurlyBracket {
                        10
                    } else {
                        1
                    };
                    self.playback
                        .seek(self.playback.frame as i64 - amount, false);
                }
                Key::CloseBracket | Key::CloseCurlyBracket => {
                    let amount = if modifiers.shift || key == Key::CloseCurlyBracket {
                        10
                    } else {
                        1
                    };
                    self.playback
                        .seek(self.playback.frame as i64 + amount, false);
                }
                Key::Semicolon if !modifiers.shift => {
                    self.playback.set_loop(self.playback.loop_mode.next());
                }
                Key::I if modifiers.shift => self.playback.in_point = 0,
                Key::Num1 if !modifiers.shift && !repeat => self.set_rotation(-90),
                Key::Num2 if !modifiers.shift && !repeat => self.set_rotation(0),
                Key::Num3 if !modifiers.shift && !repeat => self.set_rotation(90),
                Key::O => {
                    self.playback.out_point = if modifiers.shift {
                        self.playback.count.saturating_sub(1)
                    } else {
                        self.playback.frame
                    }
                }
                Key::P if !modifiers.shift => self.playback.in_out = !self.playback.in_out,
                Key::Insert => {
                    self.origin = (self.viewer.size() - self.image_size() * self.zoom) / 2.0;
                    self.fit = false;
                }
                Key::Backspace => self.fit_view(),
                Key::Delete => {
                    self.zoom = 1.0;
                    self.origin = egui::vec2(0.0, self.viewer.height() - self.image_size().y);
                    self.fit = false;
                }
                Key::Equals if !modifiers.shift => {
                    self.set_zoom(self.zoom * 2.0, self.focus(context))
                }
                Key::Minus if !modifiers.shift => {
                    self.set_zoom(self.zoom * 0.5, self.focus(context))
                }
                Key::Num0 if !modifiers.shift => self.set_zoom(1.0, self.focus(context)),
                Key::U if !modifiers.shift => self.fullscreen(context),
                Key::F if !modifiers.shift => {
                    self.fit_fill = false;
                    self.fit_view();
                }
                Key::Backtick if !modifiers.shift => self.show_ui = !self.show_ui,
                Key::R | Key::G | Key::B | Key::A if !modifiers.shift && !repeat => {
                    let channel = match key {
                        Key::R => 1,
                        Key::G => 2,
                        Key::B => 3,
                        _ => 4,
                    };
                    self.channel = if self.channel == channel { 0 } else { channel };
                }
                Key::C if !modifiers.shift => self.channel = 0,
                Key::E if modifiers.shift => {
                    self.stored = self.loader.frame(self.generation, self.playback.frame)
                }
                Key::E => self.show_stored = !self.show_stored,
                _ => {}
            }
        }
    }
}

impl eframe::App for Player {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        self.poll(context);
        if let Some(path) = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        }) {
            self.open(path);
        }
        self.timeline_mouse(context);
        self.tick();
        if self.show_ui {
            self.toolbar(context);
            self.playback_toolbar(context);
            self.timeline(context);
        } else {
            self.timeline_dragging = false;
        }
        self.viewer(context);
        self.shortcuts(context, frame.gl().map(AsRef::as_ref));
        if self.media.is_some()
            && self
                .loader
                .frame(self.generation, self.playback.frame)
                .is_none()
        {
            self.request(self.playback.frame);
        }
        if self.playback.direction != Direction::Stop || self.loading || self.picker.is_some() {
            let delay = if self.loading || self.picker.is_some() {
                Duration::from_millis(50)
            } else {
                self.playback.wait()
            };
            context.request_repaint_after(delay);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = gl
            && let Ok(mut renderer) = self.renderer.lock()
        {
            renderer.destroy(gl);
        }
    }
}

fn configure_style(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(data) = std::fs::read("/usr/share/fonts/google-noto/NotoSans-Regular.ttf") {
        fonts.font_data.insert(
            "Noto Sans".into(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "Noto Sans".into());
    }
    context.set_fonts(fonts);
    let mut style = (*context.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_gray(51);
    style.visuals.extreme_bg_color = Color32::from_gray(38);
    style.visuals.widgets.inactive.bg_fill = Color32::from_gray(64);
    style.visuals.selection.bg_fill = Color32::from_rgb(178, 153, 76);
    style.visuals.override_text_color = Some(Color32::from_gray(230));
    for font in style.text_styles.values_mut() {
        font.size = 12.0;
    }
    context.set_style(style);
}
