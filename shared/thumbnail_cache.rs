use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const THUMBNAIL_EDGE: u32 = 192;
const THUMBNAIL_VERSION: u32 = 4;

pub fn cache_directory() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".cache")
        });
    base.join("still-files/thumbnails")
}

pub fn fingerprint(path: &Path) -> Result<u64, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    Ok(metadata_fingerprint(path, &metadata))
}

pub fn metadata_fingerprint(path: &Path, metadata: &std::fs::Metadata) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .hash(&mut hasher);
    hasher.finish()
}

pub fn thumbnail_path(directory: &Path, key: u64, video: bool) -> PathBuf {
    let suffix = if video { "-video-v3-8-4" } else { "" };
    directory.join(format!(
        "{key:016x}-{THUMBNAIL_EDGE}-v{THUMBNAIL_VERSION}{suffix}.webp"
    ))
}
