use crate::instance::{OpenOptions, Request};
use std::io;
use std::path::PathBuf;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub fn request() -> io::Result<Request> {
    let mut arguments = std::env::args_os().skip(1);
    let mut root = None;
    let mut select = None;
    let mut tree = false;
    let mut sequences = false;
    let mut center = false;
    let mut configured = false;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--quit" | "--desktop" | "--no-desktop") if !configured && root.is_none() => {
                if arguments.next().is_some() {
                    return Err(invalid("This command does not accept additional arguments"));
                }
                return Ok(match argument.to_str() {
                    Some("--quit") => Request::Quit,
                    Some("--desktop") => Request::Desktop(true),
                    _ => Request::Desktop(false),
                });
            }
            Some("--root" | "--select") => {
                let value = arguments
                    .next()
                    .ok_or_else(|| invalid("Missing path after --root or --select"))?;
                if argument == "--root" {
                    if root.replace(PathBuf::from(value)).is_some() {
                        return Err(invalid("Only one root is allowed"));
                    }
                } else if select.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid("Only one selection is allowed"));
                }
                configured = true;
            }
            Some("--tree") => {
                tree = true;
                configured = true;
            }
            Some("--sequences") => {
                sequences = true;
                configured = true;
            }
            Some("--center") => {
                center = true;
                configured = true;
            }
            _ if argument.as_encoded_bytes().starts_with(b"-") => {
                return Err(invalid("Unknown command-line option"));
            }
            _ => {
                if root.replace(PathBuf::from(argument)).is_some() {
                    return Err(invalid("Only one root is allowed"));
                }
            }
        }
    }
    if !configured {
        return Ok(Request::Open(root.map(|path| {
            if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        })));
    }
    let root = root.unwrap_or(std::env::current_dir()?).canonicalize()?;
    if !root.is_dir() {
        return Err(invalid("The root must be a directory"));
    }
    let select = select
        .map(|path| root.join(path).canonicalize())
        .transpose()?;
    if select.as_ref().is_some_and(|path| !path.starts_with(&root)) {
        return Err(invalid("The selection must be inside the root"));
    }
    Ok(Request::OpenConfigured(OpenOptions {
        root,
        select,
        tree,
        sequences,
        center,
    }))
}
