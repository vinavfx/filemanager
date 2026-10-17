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
                    Action::Trash(_) => outcome.selected = None,
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
            Self::Rename { from, .. } => from,
            Self::Trash(path) | Self::Restore { path, .. } => path,
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
