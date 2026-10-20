use eframe::egui;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::OwnedFd;
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use std::collections::{HashMap, HashSet};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const REFRESH_DELAY: Duration = Duration::from_millis(350);

pub struct DirectoryWatch {
    fd: Arc<OwnedFd>,
    root: PathBuf,
    paths: HashMap<PathBuf, Option<i32>>,
    receiver: mpsc::Receiver<Instant>,
    pending: Option<Instant>,
    _stop: UnixStream,
}

impl DirectoryWatch {
    pub fn new(context: egui::Context) -> io::Result<Self> {
        let fd = Arc::new(inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?);
        let (stop, shutdown) = UnixStream::pair()?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let source = Arc::clone(&fd);
        std::thread::Builder::new()
            .name("directory-watch".into())
            .spawn(move || {
                let mut buffer = [MaybeUninit::uninit(); 8192];
                let mut reader = inotify::Reader::new(source.as_ref(), &mut buffer);
                loop {
                    let mut events = [
                        PollFd::new(source.as_ref(), PollFlags::IN),
                        PollFd::new(&shutdown, PollFlags::IN),
                    ];
                    match poll(&mut events, None) {
                        Err(rustix::io::Errno::INTR) => continue,
                        Err(error) => {
                            eprintln!("Cannot wait for directory changes: {error}");
                            break;
                        }
                        Ok(_) => {}
                    }
                    if !events[1].revents().is_empty() {
                        break;
                    }
                    if events[0]
                        .revents()
                        .intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL)
                    {
                        break;
                    }
                    let mut changed = false;
                    loop {
                        match reader.next() {
                            Ok(event) => {
                                changed |= !event.events().contains(ReadFlags::IGNORED);
                            }
                            Err(rustix::io::Errno::AGAIN) => break,
                            Err(rustix::io::Errno::INTR) => continue,
                            Err(error) => {
                                eprintln!("Cannot read directory changes: {error}");
                                return;
                            }
                        }
                        if reader.is_buffer_empty() {
                            break;
                        }
                    }
                    if changed {
                        match sender.try_send(Instant::now()) {
                            Ok(()) => context.request_repaint_after(REFRESH_DELAY),
                            Err(mpsc::TrySendError::Full(_)) => {}
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                        }
                    }
                }
            })?;
        Ok(Self {
            fd,
            root: PathBuf::new(),
            paths: HashMap::new(),
            receiver,
            pending: None,
            _stop: stop,
        })
    }

    pub fn synchronize(&mut self, root: &Path, paths: HashSet<PathBuf>, scanning: bool) {
        let keep_existing = self.root == root && scanning;
        if self.root != root {
            self.root = root.to_path_buf();
            self.pending = None;
        }
        self.paths.retain(|path, watch| {
            if keep_existing || paths.contains(path) {
                return true;
            }
            if let Some(watch) = watch {
                let _ = inotify::remove_watch(self.fd.as_ref(), *watch);
            }
            false
        });
        let flags = WatchFlags::CREATE
            | WatchFlags::DELETE
            | WatchFlags::MOVED_FROM
            | WatchFlags::MOVED_TO
            | WatchFlags::CLOSE_WRITE
            | WatchFlags::ATTRIB
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF
            | WatchFlags::ONLYDIR;
        for path in paths {
            self.paths.entry(path.clone()).or_insert_with(|| {
                match inotify::add_watch(self.fd.as_ref(), &path, flags) {
                    Ok(watch) => Some(watch),
                    Err(error) => {
                        eprintln!("Cannot watch directory {}: {error}", path.display());
                        None
                    }
                }
            });
        }
    }

    pub fn ready(&mut self, context: &egui::Context, busy: bool) -> bool {
        if let Ok(changed) = self.receiver.try_recv() {
            self.pending.get_or_insert(changed);
        }
        let Some(changed) = self.pending else {
            return false;
        };
        let remaining = REFRESH_DELAY.saturating_sub(changed.elapsed());
        if !remaining.is_zero() {
            context.request_repaint_after(remaining);
            return false;
        }
        if busy {
            return false;
        }
        self.pending = None;
        true
    }
}
