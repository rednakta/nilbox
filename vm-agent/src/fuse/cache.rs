//! Client-side metadata/directory cache with TTL expiry.

use super::protocol::{DirEntry, FileType};
use fuser::FileAttr;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_ATTR_TTL: Duration = Duration::from_secs(5);
const DEFAULT_DIR_TTL: Duration = Duration::from_secs(3);

struct CachedAttr {
    attr: FileAttr,
    cached_at: Instant,
}

struct CachedDir {
    entries: Vec<DirEntry>,
    cached_at: Instant,
}

pub struct CacheManager {
    attr_cache: HashMap<String, CachedAttr>,
    dir_cache: HashMap<String, CachedDir>,
    attr_ttl: Duration,
    dir_ttl: Duration,
}

impl CacheManager {
    pub fn new() -> Self {
        Self {
            attr_cache: HashMap::new(),
            dir_cache: HashMap::new(),
            attr_ttl: DEFAULT_ATTR_TTL,
            dir_ttl: DEFAULT_DIR_TTL,
        }
    }

    pub fn get_attr(&self, path: &str) -> Option<FileAttr> {
        self.attr_cache.get(path).and_then(|cached| {
            if cached.cached_at.elapsed() < self.attr_ttl {
                Some(cached.attr)
            } else {
                None
            }
        })
    }

    pub fn put_attr(&mut self, path: String, attr: FileAttr) {
        self.attr_cache.insert(path, CachedAttr { attr, cached_at: Instant::now() });
    }

    pub fn get_dir(&self, path: &str) -> Option<&Vec<DirEntry>> {
        self.dir_cache.get(path).and_then(|cached| {
            if cached.cached_at.elapsed() < self.dir_ttl {
                Some(&cached.entries)
            } else {
                None
            }
        })
    }

    pub fn put_dir(&mut self, path: String, entries: Vec<DirEntry>) {
        self.dir_cache.insert(path, CachedDir { entries, cached_at: Instant::now() });
    }

    pub fn invalidate(&mut self, path: &str) {
        self.attr_cache.remove(path);
        self.dir_cache.remove(path);
    }

    pub fn invalidate_with_parent(&mut self, path: &str) {
        self.invalidate(path);
        if let Some(parent) = parent_path(path) {
            self.dir_cache.remove(&parent);
        }
    }

    pub fn clear(&mut self) {
        self.attr_cache.clear();
        self.dir_cache.clear();
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

fn parent_path(path: &str) -> Option<String> {
    if path == "/" || path.is_empty() {
        return None;
    }
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(pos) => Some(path[..pos].to_string()),
        None => None,
    }
}

/// Create a fuser::FileAttr from protocol stat response data.
pub fn make_file_attr(
    ino: u64,
    file_type: FileType,
    mode: u32,
    size: u64,
    mtime: u64,
    atime: u64,
    ctime: u64,
) -> FileAttr {
    let kind = match file_type {
        FileType::Regular => fuser::FileType::RegularFile,
        FileType::Directory => fuser::FileType::Directory,
        FileType::Symlink => fuser::FileType::Symlink,
        FileType::Other => fuser::FileType::RegularFile,
    };

    FileAttr {
        ino,
        size,
        blocks: (size + 511) / 512,
        atime: UNIX_EPOCH + Duration::from_secs(atime),
        mtime: UNIX_EPOCH + Duration::from_secs(mtime),
        ctime: UNIX_EPOCH + Duration::from_secs(ctime),
        crtime: UNIX_EPOCH + Duration::from_secs(ctime),
        kind,
        perm: mode as u16,
        nlink: 1,
        uid: 0,
        gid: 0,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}
