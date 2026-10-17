use crate::files::{self, gio};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, Write};
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

enum TrashLocation {
    Known(String),
    Unresolved(HashSet<String>),
}

enum Action {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Trash(PathBuf),
    Restore {
        path: PathBuf,
        location: TrashLocation,
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
    while let Some(action) = actions.last() {
        progress(inverse.len(), total, action.path());
        match action.apply() {
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

    fn apply(&self) -> io::Result<Self> {
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
                let previous = trash_entries()?;
                gio("trash", &[], &[path.as_os_str()])?;
                let location = match find_trashed(path, &previous) {
                    Ok(uri) => TrashLocation::Known(uri),
                    Err(_) => TrashLocation::Unresolved(previous),
                };
                Ok(Self::Restore {
                    path: path.clone(),
                    location,
                })
            }
            Self::Restore { path, location } => {
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
                let uri = match location {
                    TrashLocation::Known(uri) => uri.clone(),
                    TrashLocation::Unresolved(previous) => find_trashed(path, previous)?,
                };
                if !is_original(&uri, path)? {
                    return Err(io::Error::other(
                        "The trashed item no longer matches this operation.",
                    ));
                }
                gio("move", &["-T"], &[OsStr::new(&uri), path.as_os_str()])?;
                Ok(Self::Trash(path.clone()))
            }
        }
    }
}

fn trash_entries() -> io::Result<HashSet<String>> {
    let output = gio("list", &["-u", "-h"], &[OsStr::new("trash:///")])?;
    let output = String::from_utf8(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(output.lines().map(str::to_owned).collect())
}

fn is_original(uri: &str, path: &Path) -> io::Result<bool> {
    let info = gio("info", &["-a", "trash::orig-path"], &[OsStr::new(uri)])?;
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

fn find_trashed(path: &Path, previous: &HashSet<String>) -> io::Result<String> {
    let current = trash_entries()?;
    let mut found = None;
    for uri in current.difference(previous) {
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
