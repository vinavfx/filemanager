use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub enum Request {
    Open(Option<PathBuf>),
    NavigateStill(PathBuf, bool),
    Desktop(bool),
    Quit,
}

pub struct Instance {
    _lock: File,
    listener: UnixListener,
    socket: PathBuf,
}

impl Instance {
    pub fn start(request: &Request) -> io::Result<Option<Self>> {
        let uid = rustix::process::getuid().as_raw();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("still-files-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&runtime) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(&runtime)?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "The instance directory must be private and owned by the current user",
            ));
        }
        let mut display = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        std::env::var_os("DISPLAY").hash(&mut display);
        std::env::var_os("WAYLAND_DISPLAY").hash(&mut display);
        let name = format!("{:x}", display.finish());
        let socket = runtime.join(format!("{name}.sock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(runtime.join(format!("{name}.lock")))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Self::forward(&socket, request).is_ok() {
                        return Ok(None);
                    }
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "The running instance is not responding",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        }
        if matches!(request, Request::Quit | Request::Desktop(false)) {
            return Ok(None);
        }
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&socket)?;
        Ok(Some(Self {
            _lock: lock,
            listener,
            socket,
        }))
    }

    fn forward(socket: &PathBuf, request: &Request) -> io::Result<()> {
        let mut stream = UnixStream::connect(socket)?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        match request {
            Request::Open(path) => {
                stream.write_all(&[0])?;
                if let Some(path) = path {
                    stream.write_all(path.as_os_str().as_bytes())?;
                }
            }
            Request::NavigateStill(path, forward) => {
                stream.write_all(&[if *forward { 3 } else { 2 }])?;
                stream.write_all(path.as_os_str().as_bytes())?;
            }
            Request::Desktop(enabled) => stream.write_all(&[if *enabled { 4 } else { 5 }])?,
            Request::Quit => stream.write_all(&[1])?,
        }
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut response = [0];
        stream.read_exact(&mut response)?;
        if response != [1] {
            return Err(io::Error::other(
                "The running instance rejected the request",
            ));
        }
        Ok(())
    }

    pub fn listen(&self, context: eframe::egui::Context) -> io::Result<mpsc::Receiver<Request>> {
        let listener = self.listener.try_clone()?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut stream) = connection else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut bytes = Vec::new();
                if Read::by_ref(&mut stream)
                    .take(65_537)
                    .read_to_end(&mut bytes)
                    .is_err()
                    || bytes.is_empty()
                    || bytes.len() > 65_536
                {
                    continue;
                }
                let request = match bytes[0] {
                    0 => Request::Open(if bytes.len() == 1 {
                        None
                    } else {
                        Some(PathBuf::from(std::ffi::OsString::from_vec(
                            bytes[1..].to_vec(),
                        )))
                    }),
                    2 | 3 if bytes.len() > 1 => Request::NavigateStill(
                        PathBuf::from(std::ffi::OsString::from_vec(bytes[1..].to_vec())),
                        bytes[0] == 3,
                    ),
                    4 | 5 if bytes.len() == 1 => Request::Desktop(bytes[0] == 4),
                    1 if bytes.len() == 1 => Request::Quit,
                    _ => continue,
                };
                if sender.send(request).is_err() {
                    break;
                }
                context.request_repaint();
                let _ = stream.write_all(&[1]);
            }
        });
        Ok(receiver)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}
