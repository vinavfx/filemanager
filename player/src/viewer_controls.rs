use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Clone, Copy)]
pub struct ViewerControls {
    pub values: [f32; 3],
    previous: [f32; 3],
}

impl Default for ViewerControls {
    fn default() -> Self {
        Self {
            values: [1.0; 3],
            previous: [1.0; 3],
        }
    }
}

impl ViewerControls {
    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui::Frame::NONE
            .fill(Color32::from_gray(51))
            .inner_margin(egui::Margin::symmetric(3, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.spacing_mut().button_padding = Vec2::new(3.0, 0.0);
                ui.spacing_mut().interact_size.y = 20.0;
                let width = ((ui.available_width() - 36.0) / 3.0).clamp(134.0, 223.0);
                ui.horizontal(|ui| {
                    for (index, (label, name)) in
                        [("f/8", "Gain"), ("γ", "Gamma"), ("S", "Saturation")]
                            .into_iter()
                            .enumerate()
                    {
                        if index > 0 {
                            ui.add(egui::Separator::default().spacing(12.0));
                        }
                        ui.push_id(index, |ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(width, 20.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if index == 0
                                        && ui
                                            .small_button("◀")
                                            .on_hover_text("Decrease exposure by one stop")
                                            .clicked()
                                    {
                                        self.values[0] *= 0.5;
                                    }
                                    let text = if self.values[index] != 1.0 {
                                        egui::RichText::new(label).color(Color32::RED)
                                    } else {
                                        egui::RichText::new(label)
                                    };
                                    if ui
                                        .add(egui::Button::new(text).frame(false))
                                        .on_hover_text(format!(
                                            "Toggle {name} between 1 and the last adjustment"
                                        ))
                                        .clicked()
                                    {
                                        if self.values[index] == 1.0 {
                                            self.values[index] = self.previous[index];
                                        } else {
                                            self.previous[index] = self.values[index];
                                            self.values[index] = 1.0;
                                        }
                                    }
                                    if index == 0
                                        && ui
                                            .small_button("▶")
                                            .on_hover_text("Increase exposure by one stop")
                                            .clicked()
                                    {
                                        self.values[0] = (self.values[0] * 2.0).min(f32::MAX);
                                    }
                                    let minimum = if index == 1 { 0.01 } else { 0.0 };
                                    let (input_rect, _) = ui
                                        .allocate_exact_size(Vec2::new(40.0, 20.0), Sense::hover());
                                    let mut input_ui =
                                        ui.new_child(egui::UiBuilder::new().max_rect(input_rect));
                                    input_ui.set_clip_rect(input_rect.intersect(ui.clip_rect()));
                                    let background = input_ui.visuals().extreme_bg_color;
                                    let widgets = &mut input_ui.visuals_mut().widgets;
                                    for visuals in [
                                        &mut widgets.inactive,
                                        &mut widgets.hovered,
                                        &mut widgets.active,
                                        &mut widgets.open,
                                    ] {
                                        visuals.weak_bg_fill = background;
                                        visuals.expansion = 0.0;
                                    }
                                    input_ui
                                        .add_sized(
                                            input_rect.size(),
                                            egui::DragValue::new(&mut self.values[index])
                                                .range(minimum..=f32::MAX)
                                                .speed(0.01)
                                                .custom_formatter(|value, _| format!("{value:.2}")),
                                        )
                                        .on_hover_text(format!(
                                            "{name}: drag or double-click to enter a value"
                                        ));
                                    ruler(ui, &mut self.values[index], index);
                                },
                            );
                        });
                    }
                });
            });
    }
}

fn ruler(ui: &mut egui::Ui, value: &mut f32, index: usize) {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(24.0), 20.0),
        Sense::click_and_drag(),
    );
    let track = rect.shrink2(Vec2::new(3.0, 0.0));
    let position = |value: f32| {
        if index == 0 {
            (value.max(1.0 / 64.0).log2() + 6.0) / 12.0
        } else {
            (value.max(0.0) / 4.0).sqrt()
        }
    };
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let t = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
        *value = if index == 0 {
            2.0_f32.powf(t * 12.0 - 6.0)
        } else {
            (4.0 * t * t).max(if index == 1 { 0.01 } else { 0.0 })
        };
    }
    if response.double_clicked() {
        *value = 1.0;
    }
    response.on_hover_text("Drag to adjust. Double-click to reset to 1.");
    let painter = ui.painter_at(rect);
    let baseline = rect.top() + 9.0;
    painter.line_segment(
        [
            Pos2::new(track.left(), baseline),
            Pos2::new(track.right(), baseline),
        ],
        Stroke::new(1.0, Color32::from_gray(18)),
    );
    for tick in 0..=40 {
        let x = egui::lerp(track.x_range(), tick as f32 / 40.0);
        let height = if tick % 5 == 0 { 7.0 } else { 3.0 };
        painter.line_segment(
            [Pos2::new(x, baseline - height), Pos2::new(x, baseline)],
            Stroke::new(1.0, Color32::from_gray(23)),
        );
    }
    let labels: &[(f32, &str)] = if index == 0 {
        &[
            (1.0 / 64.0, "0.015625"),
            (0.1, "0.1"),
            (1.0, "1"),
            (10.0, "10"),
            (64.0, "64"),
        ]
    } else {
        &[(0.0, "0"), (1.0, "1"), (2.0, "2"), (4.0, "4")]
    };
    let mut right = rect.left();
    for &(number, text) in labels {
        let galley = painter.layout_no_wrap(
            text.into(),
            FontId::proportional(8.0),
            Color32::from_gray(18),
        );
        let x = (track.left() + position(number) * track.width() - galley.size().x / 2.0).clamp(
            rect.left(),
            (rect.right() - galley.size().x).max(rect.left()),
        );
        if x >= right {
            right = x + galley.size().x + 2.0;
            painter.galley(Pos2::new(x, baseline + 1.0), galley, Color32::from_gray(18));
        }
    }
    let x = track.left() + position(*value).clamp(0.0, 1.0) * track.width();
    painter.rect_filled(
        Rect::from_center_size(Pos2::new(x, baseline - 1.0), Vec2::new(4.0, 10.0)),
        0.0,
        Color32::from_gray(18),
    );
    painter.line_segment(
        [Pos2::new(x, baseline - 5.0), Pos2::new(x, baseline + 3.0)],
        Stroke::new(1.0, Color32::from_rgb(178, 153, 76)),
    );
}
