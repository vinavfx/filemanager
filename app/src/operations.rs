use crate::files::{self, gio};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct History {
    undo: Vec<Vec<Action>>,
    redo: Vec<Vec<Action>>,
}

#[derive(Default)]
pub struct Outcome {
    pub changed: bool,
    pub renames: Vec<(PathBuf, PathBuf)>,
    pub copied: Vec<(PathBuf, PathBuf)>,
    pub selected: Option<PathBuf>,
    pub error: Option<String>,
}

type TrashEntries = HashMap<FileIdentity, Vec<String>>;

#[derive(Eq, Hash, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

enum Action {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    RemoveCopy {
        from: PathBuf,
        path: PathBuf,
        identity: FileIdentity,
    },
    Trash(PathBuf),
    Restore {
        path: PathBuf,
        identity: FileIdentity,
    },
}

impl History {
    pub fn record_rename(&mut self, from: PathBuf, to: PathBuf) {
        if from != to {
            self.undo.push(vec![Action::Rename { from: to, to: from }]);
            self.redo.clear();
        }
    }

    pub fn discard_redo(&mut self) {
        self.redo.clear();
    }

    pub fn rename(
        &mut self,
        paths: Vec<(PathBuf, PathBuf)>,
        progress: &mut impl FnMut(usize, usize, &Path),
    ) -> Outcome {
        let mut actions = paths
            .into_iter()
            .rev()
            .map(|(from, to)| Action::Rename { from, to })
            .collect();
        let (inverse, outcome) = execute(&mut actions, progress);
        if !inverse.is_empty() {
            self.undo.push(inverse);
            self.redo.clear();
        }
        outcome
    }

    pub fn trash(
        &mut self,
        paths: Vec<PathBuf>,
        progress: &mut impl FnMut(usize, usize, &Path),
    ) -> Outcome {
        let mut actions = paths.into_iter().rev().map(Action::Trash).collect();
        let (inverse, outcome) = execute(&mut actions, progress);
        if !inverse.is_empty() {
            self.undo.push(inverse);
            self.redo.clear();
        }
        outcome
    }

    pub fn copy(
        &mut self,
        paths: Vec<PathBuf>,
        directory: &Path,
        progress: &mut impl FnMut(usize, usize, &Path),
    ) -> Outcome {
        let mut outcome = Outcome::default();
        let mut inverse = Vec::new();
        let total = paths.len();
        for (index, from) in paths.into_iter().enumerate() {
            progress(index, total, &from);
            let result = copy_destination(&from, directory).and_then(|to| {
                let action = Action::Copy {
                    from: from.clone(),
                    to: to.clone(),
                };
                action.apply(&mut None).map(|reverse| (from, to, reverse))
            });
            match result {
                Ok((from, to, reverse)) => {
                    progress(index + 1, total, &to);
                    outcome.selected.get_or_insert_with(|| to.clone());
                    outcome.copied.push((from, to));
                    outcome.changed = true;
                    inverse.push(reverse);
                }
                Err(error) => {
                    outcome.error = Some(format!("Cannot copy files: {error}"));
                    break;
                }
            }
        }
        if !inverse.is_empty() {
            self.undo.push(inverse);
            self.redo.clear();
        }
        outcome
    }

    pub fn empty_trash(&mut self, progress: &mut impl FnMut(usize, usize, &Path)) -> Outcome {
        let path = Path::new("trash:///");
        progress(0, 1, path);
        if let Err(error) = gio("trash", &["--empty"], &[]) {
            return Outcome {
                error: Some(format!("Cannot empty trash: {error}")),
                ..Outcome::default()
            };
        }
        for stack in [&mut self.undo, &mut self.redo] {
            for actions in stack.iter_mut() {
                actions.retain(|action| !matches!(action, Action::Restore { .. }));
            }
            stack.retain(|actions| !actions.is_empty());
        }
        progress(1, 1, path);
        Outcome {
            changed: true,
            ..Outcome::default()
        }
    }

    pub fn undo(&mut self, progress: &mut impl FnMut(usize, usize, &Path)) -> Outcome {
        replay(&mut self.undo, &mut self.redo, "Nothing to undo.", progress)
    }

    pub fn redo(&mut self, progress: &mut impl FnMut(usize, usize, &Path)) -> Outcome {
        replay(&mut self.redo, &mut self.undo, "Nothing to redo.", progress)
    }
}

fn replay(
    source: &mut Vec<Vec<Action>>,
    target: &mut Vec<Vec<Action>>,
    empty: &str,
    progress: &mut impl FnMut(usize, usize, &Path),
) -> Outcome {
    let Some(mut actions) = source.pop() else {
        return Outcome {
            error: Some(empty.into()),
            ..Outcome::default()
        };
    };
    let (inverse, outcome) = execute(&mut actions, progress);
    if !actions.is_empty() {
        source.push(actions);
    }
    if !inverse.is_empty() {
        target.push(inverse);
    }
    outcome
}

fn execute(
    actions: &mut Vec<Action>,
    progress: &mut impl FnMut(usize, usize, &Path),
) -> (Vec<Action>, Outcome) {
    let mut inverse = Vec::new();
    let mut outcome = Outcome::default();
    let total = actions.len();
    let mut trash = None;
    while let Some(action) = actions.last() {
        progress(inverse.len(), total, action.path());
        match action.apply(&mut trash) {
            Ok(reverse) => {
                match action {
                    Action::Rename { from, to } => {
                        outcome.renames.push((from.clone(), to.clone()));
                        outcome.selected = Some(to.clone());
                    }
                    Action::Restore { path, .. } => outcome.selected = Some(path.clone()),
                    Action::Copy { from, to } => {
                        outcome.selected.get_or_insert_with(|| to.clone());
                        outcome.copied.push((from.clone(), to.clone()));
                    }
                    Action::Trash(_) | Action::RemoveCopy { .. } => outcome.selected = None,
                }
                outcome.changed = true;
                inverse.push(reverse);
                progress(inverse.len(), total, action.path());
                actions.pop();
            }
            Err(error) => {
                outcome.error = Some(format!(
                    "Cannot change {}: {error}",
                    action.path().display()
                ));
                break;
            }
        }
    }
    (inverse, outcome)
}

impl Action {
    fn path(&self) -> &Path {
        match self {
            Self::Rename { from, .. } | Self::Copy { from, .. } => from,
            Self::Trash(path) | Self::Restore { path, .. } | Self::RemoveCopy { path, .. } => path,
        }
    }

    fn apply(&self, trash: &mut Option<TrashEntries>) -> io::Result<Self> {
        match self {
            Self::Rename { from, to } => {
                let name = to.file_name().and_then(OsStr::to_str).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid destination name.")
                })?;
                files::rename_entry(from, name)?;
                Ok(Self::Rename {
                    from: to.clone(),
                    to: from.clone(),
                })
            }
            Self::Copy { from, to } => {
                let metadata = std::fs::symlink_metadata(from)?;
                let parent = to
                    .parent()
                    .ok_or_else(|| io::Error::other("Invalid copy destination."))?;
                if metadata.is_dir() && parent.canonicalize()?.starts_with(from.canonicalize()?) {
                    return Err(io::Error::other("Cannot copy a folder into itself."));
                }
                copy_entry(from, to)?;
                let metadata = std::fs::symlink_metadata(to)?;
                Ok(Self::RemoveCopy {
                    from: from.clone(),
                    path: to.clone(),
                    identity: FileIdentity {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    },
                })
            }
            Self::RemoveCopy {
                from,
                path,
                identity,
            } => {
                let metadata = std::fs::symlink_metadata(path)?;
                if metadata.dev() != identity.device || metadata.ino() != identity.inode {
                    return Err(io::Error::other("The copied item has been replaced."));
                }
                if metadata.is_dir() {
                    std::fs::remove_dir_all(path)?;
                } else {
                    std::fs::remove_file(path)?;
                }
                Ok(Self::Copy {
                    from: from.clone(),
                    to: path.clone(),
                })
            }
            Self::Trash(path) => {
                let metadata = std::fs::symlink_metadata(path)?;
                let identity = FileIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                };
                gio("trash", &[], &[path.as_os_str()])?;
                Ok(Self::Restore {
                    path: path.clone(),
                    identity,
                })
            }
            Self::Restore { path, identity } => {
                match std::fs::symlink_metadata(path) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "An item already exists at the original location.",
                        ))
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                let entries = match trash {
                    Some(entries) => entries,
                    None => trash.insert(trash_entries()?),
                };
                let uri = find_trashed(path, identity, entries)?;
                gio("move", &["-T"], &[OsStr::new(&uri), path.as_os_str()])?;
                Ok(Self::Trash(path.clone()))
            }
        }
    }
}

fn copy_destination(from: &Path, directory: &Path) -> io::Result<PathBuf> {
    let directory = directory.canonicalize()?;
    if !directory.is_dir() {
        return Err(io::Error::other(
            "The paste destination is not a directory.",
        ));
    }
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.is_dir() && directory.starts_with(from.canonicalize()?) {
        return Err(io::Error::other("Cannot copy a folder into itself."));
    }
    let name = from
        .file_name()
        .ok_or_else(|| io::Error::other("Invalid file name."))?;
    let mut destination = directory.join(name);
    let mut number = 1;
    while destination.try_exists()? || std::fs::symlink_metadata(&destination).is_ok() {
        let stem = if metadata.is_dir() {
            Some(name)
        } else {
            from.file_stem()
        };
        let mut name = stem.unwrap_or(name).to_os_string();
        if number == 1 {
            name.push(" (copy)");
        } else {
            name.push(format!(" (copy {number})"));
        }
        if !metadata.is_dir() {
            if let Some(extension) = from.extension() {
                name.push(".");
                name.push(extension);
            }
        }
        destination = directory.join(name);
        number += 1;
    }
    Ok(destination)
}

fn copy_entry(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
        crate::images::copy_cached_thumbnails(from, to);
        return Ok(());
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(io::Error::other(
            "Only files, folders and symbolic links can be copied.",
        ));
    }
    if metadata.is_dir() {
        std::fs::create_dir(to)?;
    } else {
        let mut source = std::fs::File::open(from)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        if let Err(error) = io::copy(&mut source, &mut target) {
            std::fs::remove_file(to)?;
            return Err(error);
        }
    }
    let result = (|| {
        if metadata.is_dir() {
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                copy_entry(&entry.path(), &to.join(entry.file_name()))?;
            }
        }
        std::fs::set_permissions(to, metadata.permissions())
    })();
    if result.is_err() {
        if metadata.is_dir() {
            std::fs::remove_dir_all(to)?;
        } else {
            std::fs::remove_file(to)?;
        }
    } else if metadata.is_file() {
        crate::images::copy_cached_thumbnails(from, to);
    }
    result
}

fn trash_entries() -> io::Result<TrashEntries> {
    let output = gio(
        "list",
        &["-u", "-h", "-n", "-a", "unix::device,unix::inode"],
        &[OsStr::new("trash:///")],
    )?;
    let output = String::from_utf8(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut entries = TrashEntries::new();
    for line in output.lines() {
        let mut columns = line.splitn(4, '\t');
        let Some(uri) = columns.next() else {
            continue;
        };
        let Some(attributes) = columns.nth(2) else {
            continue;
        };
        let attribute = |name: &str| {
            attributes
                .split_ascii_whitespace()
                .find_map(|value| value.strip_prefix(name)?.parse::<u64>().ok())
        };
        if let (Some(device), Some(inode)) = (attribute("unix::device="), attribute("unix::inode="))
        {
            entries
                .entry(FileIdentity { device, inode })
                .or_default()
                .push(uri.to_owned());
        }
    }
    Ok(entries)
}

fn is_original(uri: &str, path: &Path) -> io::Result<bool> {
    let info = gio(
        "info",
        &["-n", "-a", "trash::orig-path"],
        &[OsStr::new(uri)],
    )?;
    let mut attribute = b"\nattributes:\n  trash::orig-path: ".to_vec();
    for byte in path.as_os_str().as_encoded_bytes() {
        if (b' '..=b'~').contains(byte) && *byte != b'\\' {
            attribute.push(*byte);
        } else {
            write!(attribute, "\\x{byte:02x}")?;
        }
    }
    attribute.push(b'\n');
    Ok(info.ends_with(&attribute))
}

fn find_trashed(
    path: &Path,
    identity: &FileIdentity,
    entries: &TrashEntries,
) -> io::Result<String> {
    let mut found = None;
    for uri in entries.get(identity).into_iter().flatten() {
        if is_original(uri, path)? {
            if found.is_some() {
                return Err(io::Error::other(
                    "Multiple trashed copies match this path. Restore it from the system trash.",
                ));
            }
            found = Some(uri.clone());
        }
    }
    found.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Cannot locate the item in the system trash.",
        )
    })
}
