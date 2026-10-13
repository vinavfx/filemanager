use eframe::egui::{self, Color32, Stroke, Vec2};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub enum Kind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
}

#[derive(Clone)]
pub struct Place {
    pub path: PathBuf,
    pub label: &'static str,
    pub kind: Kind,
}

pub fn defaults(home: &Path) -> Vec<Place> {
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut places = vec![Place {
        path: home.clone(),
        label: "Home",
        kind: Kind::Home,
    }];
    for (key, label, kind) in [
        ("DESKTOP", "Desktop", Kind::Desktop),
        ("DOCUMENTS", "Documents", Kind::Documents),
        ("DOWNLOAD", "Downloads", Kind::Downloads),
        ("MUSIC", "Music", Kind::Music),
        ("PICTURES", "Pictures", Kind::Pictures),
        ("VIDEOS", "Videos", Kind::Videos),
    ] {
        let path = std::process::Command::new("xdg-user-dir")
            .arg(key)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|path| PathBuf::from(path.trim()))
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(label));
        let path = path.canonicalize().unwrap_or(path);
        if path.is_dir() && !places.iter().any(|place| place.path == path) {
            places.push(Place { path, label, kind });
        }
    }
    places
}

pub fn paint_icon(ui: &egui::Ui, rect: egui::Rect, kind: Kind, color: Color32) {
    let point = |x, y| rect.min + rect.size() * Vec2::new(x, y);
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let line = |a, b| painter.line_segment([a, b], stroke);
    match kind {
        Kind::Home => {
            line(point(0.1, 0.45), point(0.5, 0.1));
            line(point(0.5, 0.1), point(0.9, 0.45));
            line(point(0.25, 0.35), point(0.25, 0.9));
            line(point(0.25, 0.9), point(0.75, 0.9));
            line(point(0.75, 0.9), point(0.75, 0.35));
            line(point(0.5, 0.9), point(0.5, 0.6));
        }
        Kind::Downloads => {
            line(point(0.5, 0.1), point(0.5, 0.7));
            line(point(0.5, 0.7), point(0.25, 0.45));
            line(point(0.5, 0.7), point(0.75, 0.45));
            line(point(0.15, 0.85), point(0.85, 0.85));
        }
        Kind::Music => {
            line(point(0.35, 0.75), point(0.35, 0.25));
            line(point(0.35, 0.25), point(0.8, 0.1));
            line(point(0.8, 0.1), point(0.8, 0.6));
            painter.circle_filled(point(0.23, 0.78), rect.width() * 0.13, color);
            painter.circle_filled(point(0.68, 0.63), rect.width() * 0.13, color);
        }
        Kind::Desktop | Kind::Documents | Kind::Pictures | Kind::Videos => {
            let bounds = if matches!(kind, Kind::Documents) {
                egui::Rect::from_min_max(point(0.2, 0.05), point(0.8, 0.95))
            } else {
                egui::Rect::from_min_max(point(0.05, 0.15), point(0.95, 0.8))
            };
            painter.rect_stroke(bounds, 1.0, stroke, egui::StrokeKind::Inside);
            match kind {
                Kind::Desktop => {
                    line(point(0.5, 0.8), point(0.5, 0.95));
                    line(point(0.3, 0.95), point(0.7, 0.95));
                }
                Kind::Documents => {
                    for y in [0.3, 0.5, 0.7] {
                        line(point(0.35, y), point(0.65, y));
                    }
                }
                Kind::Pictures => {
                    painter.circle_filled(point(0.7, 0.35), rect.width() * 0.07, color);
                    line(point(0.15, 0.7), point(0.4, 0.4));
                    line(point(0.4, 0.4), point(0.75, 0.7));
                }
                Kind::Videos => {
                    painter.add(egui::Shape::convex_polygon(
                        vec![point(0.4, 0.3), point(0.4, 0.65), point(0.7, 0.475)],
                        color,
                        Stroke::NONE,
                    ));
                }
                _ => {}
            }
        }
    }
}
