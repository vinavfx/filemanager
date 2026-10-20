use eframe::egui;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const RESULT_LIMIT: usize = 1000;

pub fn normalize_filter(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn filter_matches(name: &str, query: &str) -> bool {
    let name = normalize_filter(name);
    let mut remaining = name.chars();
    query
        .chars()
        .all(|character| remaining.any(|candidate| candidate == character))
}

#[derive(Clone)]
pub struct Hit {
    pub path: PathBuf,
    pub label: String,
    pub directory: bool,
    pub deletion_locked: bool,
}

pub struct Update {
    generation: u64,
    pub hits: Vec<Hit>,
    pub matched: usize,
    pub indexed: usize,
    pub scanning: bool,
    pub skipped: usize,
    pub error: Option<String>,
}

struct Batch {
    hits: Vec<Hit>,
    skipped: usize,
}

pub struct Search {
    pub query: String,
    pub focus: bool,
    pub cursor: usize,
    pub reveal_cursor: bool,
    pub update: Option<Update>,
    sender: mpsc::Sender<(u64, String)>,
    receiver: mpsc::Receiver<Update>,
    generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    pub fn new(root: PathBuf, hidden: bool, context: egui::Context) -> Self {
        let (sender, commands) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(1));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_generation = Arc::clone(&generation);
        std::thread::spawn(move || {
            run(
                root,
                hidden,
                context,
                commands,
                updates,
                worker_generation,
                worker_cancel,
            );
        });
        Self {
            query: String::new(),
            focus: true,
            cursor: 0,
            reveal_cursor: true,
            update: None,
            sender,
            receiver,
            generation,
            cancel,
        }
    }

    pub fn submit(&mut self) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sender.send((generation, self.query.clone()));
    }

    pub fn poll(&mut self) {
        while let Ok(update) = self.receiver.try_recv() {
            if update.generation == self.generation.load(Ordering::Relaxed) {
                if self
                    .update
                    .as_ref()
                    .is_none_or(|previous| previous.generation != update.generation)
                {
                    self.cursor = 0;
                    self.reveal_cursor = true;
                } else {
                    self.cursor = self.cursor.min(update.hits.len().saturating_sub(1));
                }
                self.update = Some(update);
            }
        }
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn scan(root: PathBuf, hidden: bool, cancel: Arc<AtomicBool>, sender: mpsc::SyncSender<Batch>) {
    let mut pending = vec![root.clone()];
    let mut batch = Batch {
        hits: Vec::new(),
        skipped: 0,
    };
    let mut last_send = Instant::now();
    while let Some(directory) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => {
                batch.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    batch.skipped += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            if !hidden && (name.starts_with(b".") || name.ends_with(b"~")) {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    batch.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path.clone());
            }
            let label = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            batch.hits.push(Hit {
                deletion_locked: crate::files::deletion_locked(&path),
                path,
                label,
                directory: kind.is_dir(),
            });
            if batch.hits.len() >= 2048 || last_send.elapsed() >= Duration::from_millis(50) {
                if sender.send(batch).is_err() {
                    return;
                }
                batch = Batch {
                    hits: Vec::new(),
                    skipped: 0,
                };
                last_send = Instant::now();
            }
        }
    }
    let _ = sender.send(batch);
}

fn run(
    root: PathBuf,
    hidden: bool,
    context: egui::Context,
    commands: mpsc::Receiver<(u64, String)>,
    updates: mpsc::Sender<Update>,
    generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
) {
    let (sender, batches) = mpsc::sync_channel(4);
    let scanner_cancel = Arc::clone(&cancel);
    std::thread::spawn(move || scan(root, hidden, scanner_cancel, sender));
    let mut hits = Vec::new();
    let mut input = Arc::new(Vec::new());
    let mut query = String::new();
    let mut revision = 1;
    let mut scanning = true;
    let mut skipped = 0;
    let mut dirty = true;
    let mut last_search = Instant::now() - Duration::from_secs(1);
    while !cancel.load(Ordering::Relaxed) {
        let mut changed_query = false;
        while let Ok((next_revision, next_query)) = commands.try_recv() {
            revision = next_revision;
            query = next_query;
            dirty = true;
            changed_query = true;
        }
        for _ in 0..8 {
            match batches.try_recv() {
                Ok(batch) => {
                    skipped += batch.skipped;
                    let bytes = Arc::make_mut(&mut input);
                    for hit in batch.hits {
                        bytes.extend_from_slice(
                            format!("{}\t{}\0", hits.len(), hit.label).as_bytes(),
                        );
                        hits.push(hit);
                    }
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if scanning {
                        scanning = false;
                        dirty = true;
                    }
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if dirty
            && (changed_query || !scanning || last_search.elapsed() >= Duration::from_millis(100))
        {
            let result = if query.is_empty() {
                Ok(Some((
                    (0..hits.len().min(RESULT_LIMIT)).collect(),
                    hits.len(),
                )))
            } else {
                filter(Arc::clone(&input), &query, revision, &generation, &cancel)
            };
            let (indices, matched, error) = match result {
                Ok(Some((indices, matched))) => (indices, matched, None),
                Ok(None) => continue,
                Err(error) => (Vec::new(), 0, Some(format!("Cannot search: {error}"))),
            };
            let update = Update {
                generation: revision,
                hits: indices
                    .into_iter()
                    .filter_map(|index| hits.get(index).cloned())
                    .collect(),
                matched,
                indexed: hits.len(),
                scanning,
                skipped,
                error,
            };
            if updates.send(update).is_err() {
                return;
            }
            context.request_repaint();
            last_search = Instant::now();
            dirty = false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

type Matches = Option<(Vec<usize>, usize)>;

fn filter(
    input: Arc<Vec<u8>>,
    query: &str,
    revision: u64,
    generation: &AtomicU64,
    cancel: &AtomicBool,
) -> io::Result<Matches> {
    let mut command = Command::new("fzf");
    configure_filter(&mut command, query);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let executable = std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".fzf/bin/fzf"))
                .filter(|path| path.is_file())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "fzf was not found. Add it to PATH or install it in ~/.fzf/bin.",
                    )
                })?;
            let mut command = Command::new(executable);
            configure_filter(&mut command, query);
            command.spawn()?
        }
        Err(error) => return Err(error),
    };
    collect_matches(&mut child, input, revision, generation, cancel)
}

fn configure_filter(command: &mut Command, query: &str) {
    command
        .env_remove("FZF_DEFAULT_OPTS")
        .env_remove("FZF_DEFAULT_OPTS_FILE")
        .args([
            "--read0",
            "--print0",
            "--delimiter=\t",
            "--with-nth=2..",
            "--filter",
            query,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
}

fn collect_matches(
    child: &mut std::process::Child,
    input: Arc<Vec<u8>>,
    revision: u64,
    generation: &AtomicU64,
    cancel: &AtomicBool,
) -> io::Result<Matches> {
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let writer = std::thread::spawn(move || -> io::Result<()> {
        if let Some(mut stdin) = stdin {
            stdin.write_all(&input)?;
        }
        Ok(())
    });
    let reader = std::thread::spawn(move || -> io::Result<(Vec<usize>, usize)> {
        let mut indices = Vec::new();
        let mut count = 0;
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout);
            let mut record = Vec::new();
            while reader.read_until(0, &mut record)? != 0 {
                if let Some(end) = record.iter().position(|byte| *byte == b'\t') {
                    if let Ok(index) = String::from_utf8_lossy(&record[..end]).parse::<usize>() {
                        count += 1;
                        if indices.len() < RESULT_LIMIT {
                            indices.push(index);
                        }
                    }
                }
                record.clear();
            }
        }
        Ok((indices, count))
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) || generation.load(Ordering::Relaxed) != revision {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            let _ = reader.join();
            return Ok(None);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = reader.join();
                return Err(error);
            }
        }
    };
    let written = writer
        .join()
        .map_err(|_| io::Error::other("Search input worker failed."))?;
    let result = reader
        .join()
        .map_err(|_| io::Error::other("Search output worker failed."))?;
    if !status.success() && status.code() != Some(1) {
        return Err(io::Error::other(format!(
            "Search engine exited with {status}."
        )));
    }
    written?;
    result.map(Some)
}
