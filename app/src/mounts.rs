use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Kind {
    Network,
    External,
}

pub struct Mounts {
    entries: Vec<(PathBuf, Option<Kind>)>,
    checked: Instant,
}

impl Mounts {
    pub fn new() -> Self {
        let mut mounts = Self {
            entries: Vec::new(),
            checked: Instant::now(),
        };
        mounts.reload();
        mounts
    }

    pub fn poll(&mut self) {
        if self.checked.elapsed() >= Duration::from_secs(2) {
            self.checked = Instant::now();
            self.reload();
        }
    }

    fn reload(&mut self) {
        let Ok(content) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return;
        };
        self.entries = content
            .lines()
            .filter_map(|line| {
                let (mount, filesystem) = line.split_once(" - ")?;
                let path = mount_path(mount.split_whitespace().nth(4)?);
                let mut fields = filesystem.split_whitespace();
                let filesystem = fields.next()?;
                let source = fields.next()?;
                let kind = if matches!(
                    filesystem,
                    "cifs"
                        | "smb3"
                        | "nfs"
                        | "nfs4"
                        | "ceph"
                        | "9p"
                        | "davfs"
                        | "davfs2"
                        | "fuse.sshfs"
                        | "fuse.rclone"
                        | "fuse.davfs"
                        | "fuse.glusterfs"
                        | "fuse.gvfsd-fuse"
                ) || source.starts_with("//")
                {
                    Some(Kind::Network)
                } else if path.starts_with("/media")
                    || path.starts_with("/run/media")
                    || path.starts_with("/mnt") && source.starts_with("/dev/")
                {
                    Some(Kind::External)
                } else {
                    None
                };
                Some((path, kind))
            })
            .collect();
        self.entries
            .sort_by_key(|(path, _)| std::cmp::Reverse(path.as_os_str().len()));
    }

    pub fn kind(&self, path: &Path) -> Option<Kind> {
        self.entries
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .and_then(|(_, kind)| *kind)
    }
}

fn mount_path(value: &str) -> PathBuf {
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            let digits: Vec<_> = bytes.by_ref().take(3).collect();
            if digits.len() == 3
                && matches!(digits[0], b'0'..=b'3')
                && digits[1..].iter().all(|digit| matches!(digit, b'0'..=b'7'))
            {
                decoded.push((digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + digits[2] - b'0');
            } else {
                decoded.push(byte);
                decoded.extend(digits);
            }
        } else {
            decoded.push(byte);
        }
    }
    PathBuf::from(OsString::from_vec(decoded))
}
