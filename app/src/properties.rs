use eframe::egui;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

struct Information {
    metadata: fs::Metadata,
    target: Option<PathBuf>,
    modified: String,
}

struct Application {
    id: String,
    name: String,
    path: PathBuf,
}

struct Associations {
    mime: String,
    default: String,
    applications: Vec<Application>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Basic,
    Permissions,
    OpenWith,
}

pub struct Properties {
    path: PathBuf,
    tab: Tab,
    information: Option<Information>,
    information_job: mpsc::Receiver<Result<Information, String>>,
    associations: Option<Associations>,
    associations_job: Option<mpsc::Receiver<Result<Associations, String>>>,
    action: Option<mpsc::Receiver<Result<String, String>>>,
    mode: u32,
    selected: usize,
    error: Option<String>,
    notice: Option<String>,
}

fn background<T: Send + 'static>(
    context: &egui::Context,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> mpsc::Receiver<Result<T, String>> {
    let (sender, receiver) = mpsc::channel();
    let context = context.clone();
    std::thread::spawn(move || {
        let _ = sender.send(work());
        context.request_repaint();
    });
    receiver
}

fn output(command: &mut Command) -> Result<String, String> {
    let result = command
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr).trim().to_owned();
        return Err(if error.is_empty() {
            format!("Command failed: {}", result.status)
        } else {
            error
        });
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn associations(path: &Path) -> Result<Associations, String> {
    let mime = if path.is_dir() {
        "inode/directory".to_owned()
    } else {
        output(
            Command::new("xdg-mime")
                .args(["query", "filetype"])
                .arg(path),
        )?
    };
    let default = output(Command::new("xdg-mime").args(["query", "default", &mime]))?;
    let registered = output(Command::new("gio").args(["mime", &mime]))?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut directories = vec![std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))];
    directories.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    let mut applications = Vec::<Application>::new();
    for id in std::iter::once(default.as_str()).chain(registered.lines().map(str::trim)) {
        if !id.ends_with(".desktop") || applications.iter().any(|app| app.id == id) {
            continue;
        }
        let Some((path, contents)) = directories.iter().find_map(|directory| {
            let path = directory.join("applications").join(id);
            fs::read_to_string(&path)
                .ok()
                .map(|contents| (path, contents))
        }) else {
            continue;
        };
        let section = contents.split("[Desktop Entry]").nth(1).unwrap_or("");
        let name = section
            .lines()
            .take_while(|line| !line.starts_with('['))
            .find_map(|line| line.strip_prefix("Name="))
            .unwrap_or(id)
            .to_owned();
        applications.push(Application {
            id: id.to_owned(),
            name,
            path,
        });
    }
    applications.sort_by_cached_key(|app| app.name.to_lowercase());
    Ok(Associations {
        mime,
        default,
        applications,
    })
}

impl Properties {
    pub fn new(path: &Path, context: &egui::Context) -> Self {
        let source = path.to_path_buf();
        let information_job = background(context, move || {
            let metadata = fs::symlink_metadata(&source).map_err(|e| e.to_string())?;
            let target = fs::read_link(&source).ok();
            let modified = output(Command::new("date").args([
                "-d",
                &format!("@{}", metadata.mtime()),
                "+%Y-%m-%d %H:%M:%S %Z",
            ]))
            .unwrap_or_else(|_| metadata.mtime().to_string());
            Ok(Information {
                metadata,
                target,
                modified,
            })
        });
        Self {
            path: path.to_path_buf(),
            tab: Tab::Basic,
            information: None,
            information_job,
            associations: None,
            associations_job: None,
            action: None,
            mode: 0,
            selected: 0,
            error: None,
            notice: None,
        }
    }

    pub fn show(&mut self, context: &egui::Context) -> bool {
        if let Ok(result) = self.information_job.try_recv() {
            match result {
                Ok(information) => {
                    self.mode = information.metadata.mode();
                    self.information = Some(information);
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(job) = &self.associations_job {
            if let Ok(result) = job.try_recv() {
                match result {
                    Ok(associations) => {
                        self.associations_job = None;
                        self.selected = associations
                            .applications
                            .iter()
                            .position(|app| app.id == associations.default)
                            .unwrap_or(0);
                        self.associations = Some(associations);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if let Some(job) = &self.action {
            if let Ok(result) = job.try_recv() {
                self.action = None;
                match result {
                    Ok(message) => {
                        if message == "Default application updated." {
                            let path = self.path.clone();
                            self.associations_job =
                                Some(background(context, move || associations(&path)));
                        }
                        self.notice = Some(message);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        let mut close = false;
        let frame = egui::Frame::window(&context.style())
            .inner_margin(18)
            .corner_radius(10);
        let response = egui::Modal::new(egui::Id::new("file-properties"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(64))
            .show(context, |ui| {
                ui.set_width((context.screen_rect().width() - 72.0).clamp(240.0, 440.0));
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                ui.label(
                    egui::RichText::new("Properties")
                        .size(12.0)
                        .color(ui.visuals().text_color()),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            self.path
                                .file_name()
                                .unwrap_or(self.path.as_os_str())
                                .to_string_lossy(),
                        )
                        .size(18.0)
                        .strong(),
                    )
                    .truncate(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Basic, "Basic");
                    ui.selectable_value(&mut self.tab, Tab::Permissions, "Permissions");
                    ui.selectable_value(&mut self.tab, Tab::OpenWith, "Open With");
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("properties-content")
                    .max_height((context.screen_rect().height() - 220.0).clamp(100.0, 190.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        Tab::Basic => self.basic(ui),
                        Tab::Permissions => self.permissions(ui, context),
                        Tab::OpenWith => self.open_with(ui, context),
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    let message = self
                        .error
                        .as_deref()
                        .or(self.notice.as_deref())
                        .unwrap_or("");
                    let color = if self.error.is_some() {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.add_sized(
                        [(ui.available_width() - 88.0).max(0.0), 32.0],
                        egui::Label::new(egui::RichText::new(message).color(color))
                            .truncate()
                            .halign(egui::Align::LEFT),
                    )
                    .on_hover_text(message);
                    close = ui
                        .add_sized([80.0, 32.0], egui::Button::new("Close"))
                        .clicked();
                });
            });
        close || response.should_close()
    }

    fn basic(&self, ui: &mut egui::Ui) {
        let Some(info) = &self.information else {
            ui.label("Loading information…");
            return;
        };
        let metadata = &info.metadata;
        let value_width = (ui.available_width() - 112.0).max(100.0);
        egui::Grid::new("properties-basic")
            .min_col_width(96.0)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for (label, value) in [
                    (
                        "Location",
                        self.path
                            .parent()
                            .unwrap_or(&self.path)
                            .display()
                            .to_string(),
                    ),
                    (
                        "Type",
                        if metadata.is_symlink() {
                            "Symbolic link"
                        } else if metadata.is_dir() {
                            "Folder"
                        } else {
                            "File"
                        }
                        .to_owned(),
                    ),
                    (
                        "Size",
                        if metadata.is_dir() {
                            "Not calculated".to_owned()
                        } else if metadata.len() < 1024 {
                            format!("{} bytes", metadata.len())
                        } else {
                            format!(
                                "{} ({} bytes)",
                                crate::files::size_label(metadata.len()),
                                metadata.len()
                            )
                        },
                    ),
                    ("Modified", info.modified.clone()),
                    (
                        "Owner / Group",
                        format!("{} / {}", metadata.uid(), metadata.gid()),
                    ),
                ] {
                    ui.label(label);
                    ui.allocate_ui_with_layout(
                        egui::vec2(value_width, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&value)
                                        .color(ui.visuals().strong_text_color()),
                                )
                                .truncate()
                                .selectable(true),
                            )
                            .on_hover_text(&value);
                        },
                    );
                    ui.end_row();
                }
                if let Some(target) = &info.target {
                    ui.label("Link target");
                    let target = target.display().to_string();
                    ui.allocate_ui_with_layout(
                        egui::vec2(value_width, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(egui::Label::new(&target).truncate().selectable(true))
                                .on_hover_text(&target);
                        },
                    );
                    ui.end_row();
                }
            });
    }

    fn permissions(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let Some(info) = &self.information else {
            ui.label("Loading permissions…");
            return;
        };
        let symlink = info.metadata.is_symlink();
        ui.label(format!("Mode: {:04o}", self.mode & 0o7777));
        ui.add_enabled_ui(!symlink && self.action.is_none(), |ui| {
            egui::Grid::new("properties-permissions").show(ui, |ui| {
                ui.label("");
                for label in ["Read", "Write", "Execute"] {
                    ui.label(label);
                }
                ui.end_row();
                for (label, shift) in [("Owner", 6), ("Group", 3), ("Others", 0)] {
                    ui.label(label);
                    for bit in [4, 2, 1] {
                        let mask = bit << shift;
                        let mut enabled = self.mode & mask != 0;
                        if ui.checkbox(&mut enabled, "").changed() {
                            if enabled {
                                self.mode |= mask;
                            } else {
                                self.mode &= !mask;
                            }
                        }
                    }
                    ui.end_row();
                }
            });
            ui.add_space(12.0);
            if ui.button("Apply Permissions").clicked() {
                let path = self.path.clone();
                let mode = self.mode & 0o7777;
                self.error = None;
                self.notice = None;
                self.action = Some(background(context, move || {
                    fs::set_permissions(path, fs::Permissions::from_mode(mode))
                        .map_err(|e| e.to_string())?;
                    Ok("Permissions updated.".to_owned())
                }));
            }
        });
        if symlink {
            ui.label("Permissions cannot be edited for symbolic links.");
        }
        ui.label("Changes apply only to this item, not its contents.");
    }

    fn open_with(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if self.associations.is_none() && self.associations_job.is_none() {
            let path = self.path.clone();
            self.associations_job = Some(background(context, move || associations(&path)));
        }
        let Some(associations) = &self.associations else {
            ui.label("Loading applications…");
            return;
        };
        ui.label(format!("File type: {}", associations.mime));
        ui.add_enabled_ui(self.action.is_none(), |ui| {
            for (index, application) in associations.applications.iter().enumerate() {
                let label = if application.id == associations.default {
                    format!("{} (default)", application.name)
                } else {
                    application.name.clone()
                };
                ui.selectable_value(&mut self.selected, index, label);
            }
            let Some(application) = associations.applications.get(self.selected) else {
                ui.label("No applications registered for this file type.");
                return;
            };
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() {
                    self.error = None;
                    self.notice = None;
                    let desktop = application.path.clone();
                    let path = self.path.clone();
                    self.action = Some(background(context, move || {
                        output(Command::new("gio").arg("launch").arg(desktop).arg(path))?;
                        Ok("Application launched.".to_owned())
                    }));
                }
                if ui.button("Set as Default").clicked() {
                    self.error = None;
                    self.notice = None;
                    let id = application.id.clone();
                    let mime = associations.mime.clone();
                    self.action = Some(background(context, move || {
                        output(Command::new("xdg-mime").args(["default", &id, &mime]))?;
                        Ok("Default application updated.".to_owned())
                    }));
                }
            });
        });
    }
}
