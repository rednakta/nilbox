//! HostFilesystem — fuser::Filesystem trait implementation.
//!
//! Proxies kernel FUSE requests to host via RequestDispatcher.

use super::cache::{make_file_attr, CacheManager};
use super::dispatcher::{DispatcherError, RequestDispatcher};
use super::protocol::{status_to_errno, FileType, FuseNotification, FuseResponse, StatusCode};
use fuser::{
    FileAttr, FileType as FuserFileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{EIO, ENOENT};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tracing::{debug, error};

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

/// inode <-> path bidirectional mapping
struct InodeMap {
    ino_to_path: HashMap<u64, String>,
    path_to_ino: HashMap<String, u64>,
    next_ino: AtomicU64,
}

impl InodeMap {
    fn new() -> Self {
        let mut map = Self {
            ino_to_path: HashMap::new(),
            path_to_ino: HashMap::new(),
            next_ino: AtomicU64::new(2), // 1 reserved for root
        };
        map.ino_to_path.insert(ROOT_INO, "/".to_string());
        map.path_to_ino.insert("/".to_string(), ROOT_INO);
        map
    }

    fn get_or_create(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.path_to_ino.get(path) {
            return ino;
        }
        let ino = self.next_ino.fetch_add(1, Ordering::SeqCst);
        self.ino_to_path.insert(ino, path.to_string());
        self.path_to_ino.insert(path.to_string(), ino);
        ino
    }

    fn get_path(&self, ino: u64) -> Option<&String> {
        self.ino_to_path.get(&ino)
    }

    fn remove(&mut self, path: &str) {
        if let Some(ino) = self.path_to_ino.remove(path) {
            self.ino_to_path.remove(&ino);
        }
    }
}

/// Local fh <-> remote fd mapping
struct HandleManager {
    local_to_remote: HashMap<u64, u64>,
    next_fh: AtomicU64,
}

impl HandleManager {
    fn new() -> Self {
        Self {
            local_to_remote: HashMap::new(),
            next_fh: AtomicU64::new(1),
        }
    }

    fn allocate(&mut self, remote_fd: u64) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::SeqCst);
        self.local_to_remote.insert(fh, remote_fd);
        fh
    }

    fn get_remote(&self, fh: u64) -> Option<u64> {
        self.local_to_remote.get(&fh).copied()
    }

    fn release(&mut self, fh: u64) -> Option<u64> {
        self.local_to_remote.remove(&fh)
    }
}

pub struct HostFilesystem {
    dispatcher: Arc<RequestDispatcher>,
    handles: Arc<Mutex<HandleManager>>,
    cache: Arc<Mutex<CacheManager>>,
    inodes: Arc<Mutex<InodeMap>>,
    rt: Handle,
}

impl HostFilesystem {
    pub fn new(dispatcher: Arc<RequestDispatcher>, rt: Handle) -> Self {
        Self {
            dispatcher,
            handles: Arc::new(Mutex::new(HandleManager::new())),
            cache: Arc::new(Mutex::new(CacheManager::new())),
            inodes: Arc::new(Mutex::new(InodeMap::new())),
            rt,
        }
    }

    /// Setup notification handler for path changes and cache invalidation.
    pub async fn setup_notification_handler(&self) {
        let cache = self.cache.clone();
        let handler = Box::new(move |notification: FuseNotification| {
            let cache = cache.clone();
            tokio::spawn(async move {
                match notification {
                    FuseNotification::PathChanged { old_path, new_path } => {
                        debug!("PATH_CHANGED: {} -> {}", old_path, new_path);
                        let mut c = cache.lock().await;
                        c.clear();
                    }
                    FuseNotification::InvalidateCache => {
                        debug!("INVALIDATE_CACHE received");
                        let mut c = cache.lock().await;
                        c.clear();
                    }
                    FuseNotification::PathPending { pending_handles, timeout_sec } => {
                        debug!("PATH_PENDING: {} handles, timeout={}s", pending_handles, timeout_sec);
                    }
                    FuseNotification::PathReady => {
                        debug!("PATH_READY received");
                    }
                    FuseNotification::Shutdown => {
                        debug!("SHUTDOWN notification received");
                    }
                }
            });
        });
        self.dispatcher.set_notification_handler(handler).await;
    }

    fn build_path(parent_path: &str, name: &OsStr) -> String {
        let name_str = name.to_string_lossy();
        if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        }
    }

    fn err_to_errno(err: &DispatcherError) -> i32 {
        match err {
            DispatcherError::StatusError(status) => status_to_errno(*status),
            DispatcherError::Timeout => libc::ETIMEDOUT,
            _ => EIO,
        }
    }
}

impl Filesystem for HostFilesystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let name = name.to_owned();

        self.rt.spawn(async move {
            let parent_path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(parent) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            let path = Self::build_path(&parent_path, &name);

            // Check cache
            {
                let cache = cache.lock().await;
                if let Some(attr) = cache.get_attr(&path) {
                    reply.entry(&TTL, &attr, 0);
                    return;
                }
            }

            match dispatcher.stat(path.clone()).await {
                Ok(FuseResponse::Stat { file_type, mode, size, mtime, atime, ctime, .. }) => {
                    let ino = {
                        let mut inodes = inodes.lock().await;
                        inodes.get_or_create(&path)
                    };
                    let attr = make_file_attr(ino, file_type, mode, size, mtime, atime, ctime);
                    {
                        let mut cache = cache.lock().await;
                        cache.put_attr(path, attr);
                    }
                    reply.entry(&TTL, &attr, 0);
                }
                Ok(_) => { reply.error(EIO); }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();

        self.rt.spawn(async move {
            let path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(ino) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            {
                let cache = cache.lock().await;
                if let Some(attr) = cache.get_attr(&path) {
                    reply.attr(&TTL, &attr);
                    return;
                }
            }

            match dispatcher.stat(path.clone()).await {
                Ok(FuseResponse::Stat { file_type, mode, size, mtime, atime, ctime, .. }) => {
                    let attr = make_file_attr(ino, file_type, mode, size, mtime, atime, ctime);
                    {
                        let mut cache = cache.lock().await;
                        cache.put_attr(path, attr);
                    }
                    reply.attr(&TTL, &attr);
                }
                Ok(_) => { reply.error(EIO); }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();
        let inodes = self.inodes.clone();

        self.rt.spawn(async move {
            let path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(ino) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            match dispatcher.open(path, flags as u32, 0).await {
                Ok(fd) => {
                    let mut h = handles.lock().await;
                    let fh = h.allocate(fd);
                    reply.opened(fh, 0);
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn read(
        &mut self, _req: &Request, _ino: u64, fh: u64, offset: i64,
        size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData,
    ) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();

        self.rt.spawn(async move {
            let remote_fd = {
                let h = handles.lock().await;
                match h.get_remote(fh) {
                    Some(fd) => fd,
                    None => { reply.error(ENOENT); return; }
                }
            };

            match dispatcher.read(remote_fd, offset as u64, size).await {
                Ok(data) => { reply.data(&data); }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn write(
        &mut self, _req: &Request, ino: u64, fh: u64, offset: i64,
        data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let data = data.to_vec();

        self.rt.spawn(async move {
            let remote_fd = {
                let h = handles.lock().await;
                match h.get_remote(fh) {
                    Some(fd) => fd,
                    None => { reply.error(ENOENT); return; }
                }
            };

            match dispatcher.write(remote_fd, offset as u64, data).await {
                Ok(written) => {
                    // Invalidate cache for this file
                    {
                        let path = {
                            let inodes = inodes.lock().await;
                            inodes.get_path(ino).cloned()
                        };
                        if let Some(path) = path {
                            let mut c = cache.lock().await;
                            c.invalidate(&path);
                        }
                    }
                    reply.written(written);
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn release(
        &mut self, _req: &Request, _ino: u64, fh: u64, _flags: i32,
        _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty,
    ) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();

        self.rt.spawn(async move {
            let remote_fd = {
                let mut h = handles.lock().await;
                h.release(fh)
            };

            if let Some(fd) = remote_fd {
                match dispatcher.close(fd).await {
                    Ok(()) => { reply.ok(); }
                    Err(e) => {
                        error!("Close error: {:?}", e);
                        reply.ok(); // Still report success to FUSE
                    }
                }
            } else {
                reply.ok();
            }
        });
    }

    fn readdir(
        &mut self, _req: &Request, ino: u64, fh: u64, offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();

        self.rt.spawn(async move {
            let (path, remote_fd) = {
                let inodes = inodes.lock().await;
                let h = handles.lock().await;
                let path = match inodes.get_path(ino) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                };
                let fd = match h.get_remote(fh) {
                    Some(fd) => fd,
                    None => { reply.error(ENOENT); return; }
                };
                (path, fd)
            };

            // Check cache
            {
                let cache_guard = cache.lock().await;
                if let Some(entries) = cache_guard.get_dir(&path) {
                    let mut idx = offset as usize;
                    for entry in entries.iter().skip(idx) {
                        let file_type = match entry.file_type {
                            FileType::Directory => FuserFileType::Directory,
                            FileType::Symlink => FuserFileType::Symlink,
                            _ => FuserFileType::RegularFile,
                        };
                        let entry_path = Self::build_path(&path, OsStr::new(&entry.name));
                        let entry_ino = {
                            let mut inodes = inodes.lock().await;
                            inodes.get_or_create(&entry_path)
                        };
                        idx += 1;
                        if reply.add(entry_ino, idx as i64, file_type, &entry.name) {
                            break;
                        }
                    }
                    reply.ok();
                    return;
                }
            }

            match dispatcher.readdir(remote_fd, offset as u64, 100).await {
                Ok(entries) => {
                    {
                        let mut cache = cache.lock().await;
                        cache.put_dir(path.clone(), entries.clone());
                    }

                    let mut idx = offset as usize;
                    for entry in entries.iter().skip(idx) {
                        let file_type = match entry.file_type {
                            FileType::Directory => FuserFileType::Directory,
                            FileType::Symlink => FuserFileType::Symlink,
                            _ => FuserFileType::RegularFile,
                        };
                        let entry_path = Self::build_path(&path, OsStr::new(&entry.name));
                        let entry_ino = {
                            let mut inodes = inodes.lock().await;
                            inodes.get_or_create(&entry_path)
                        };
                        idx += 1;
                        if reply.add(entry_ino, idx as i64, file_type, &entry.name) {
                            break;
                        }
                    }
                    reply.ok();
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn opendir(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();
        let inodes = self.inodes.clone();

        self.rt.spawn(async move {
            let path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(ino) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            match dispatcher.open(path, 0, 0).await {
                Ok(fd) => {
                    let mut h = handles.lock().await;
                    let fh = h.allocate(fd);
                    reply.opened(fh, 0);
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn releasedir(&mut self, _req: &Request, _ino: u64, fh: u64, _flags: i32, reply: ReplyEmpty) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();

        self.rt.spawn(async move {
            let remote_fd = {
                let mut h = handles.lock().await;
                h.release(fh)
            };
            if let Some(fd) = remote_fd {
                let _ = dispatcher.close(fd).await;
            }
            reply.ok();
        });
    }

    fn mkdir(
        &mut self, _req: &Request, parent: u64, name: &OsStr,
        mode: u32, _umask: u32, reply: ReplyEntry,
    ) {
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let name = name.to_owned();

        self.rt.spawn(async move {
            let parent_path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(parent) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            let path = Self::build_path(&parent_path, &name);

            match dispatcher.mkdir(path.clone(), mode).await {
                Ok(()) => {
                    {
                        let mut c = cache.lock().await;
                        c.invalidate_with_parent(&path);
                    }
                    match dispatcher.stat(path.clone()).await {
                        Ok(FuseResponse::Stat { file_type, mode, size, mtime, atime, ctime, .. }) => {
                            let ino = {
                                let mut inodes = inodes.lock().await;
                                inodes.get_or_create(&path)
                            };
                            let attr = make_file_attr(ino, file_type, mode, size, mtime, atime, ctime);
                            reply.entry(&TTL, &attr, 0);
                        }
                        _ => { reply.error(EIO); }
                    }
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn create(
        &mut self, _req: &Request, parent: u64, name: &OsStr,
        mode: u32, _umask: u32, flags: i32, reply: ReplyCreate,
    ) {
        let dispatcher = self.dispatcher.clone();
        let handles = self.handles.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let name = name.to_owned();

        self.rt.spawn(async move {
            let parent_path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(parent) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            let path = Self::build_path(&parent_path, &name);
            let open_flags = flags as u32 | 0x40; // O_CREAT

            match dispatcher.open(path.clone(), open_flags, mode).await {
                Ok(fd) => {
                    let ino = {
                        let mut inodes = inodes.lock().await;
                        inodes.get_or_create(&path)
                    };
                    {
                        let mut c = cache.lock().await;
                        c.invalidate_with_parent(&path);
                    }
                    let mut h = handles.lock().await;
                    let fh = h.allocate(fd);

                    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                    let attr = FileAttr {
                        ino,
                        size: 0,
                        blocks: 0,
                        atime: UNIX_EPOCH + Duration::from_secs(now),
                        mtime: UNIX_EPOCH + Duration::from_secs(now),
                        ctime: UNIX_EPOCH + Duration::from_secs(now),
                        crtime: UNIX_EPOCH + Duration::from_secs(now),
                        kind: FuserFileType::RegularFile,
                        perm: (mode & 0o7777) as u16,
                        nlink: 1,
                        uid: 0,
                        gid: 0,
                        rdev: 0,
                        blksize: 4096,
                        flags: 0,
                    };
                    reply.created(&TTL, &attr, 0, fh, 0);
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let name = name.to_owned();

        self.rt.spawn(async move {
            let parent_path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(parent) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            let path = Self::build_path(&parent_path, &name);

            match dispatcher.remove(path.clone(), false).await {
                Ok(()) => {
                    {
                        let mut c = cache.lock().await;
                        c.invalidate_with_parent(&path);
                    }
                    {
                        let mut inodes = inodes.lock().await;
                        inodes.remove(&path);
                    }
                    reply.ok();
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();
        let name = name.to_owned();

        self.rt.spawn(async move {
            let parent_path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(parent) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            let path = Self::build_path(&parent_path, &name);

            match dispatcher.remove(path.clone(), true).await {
                Ok(()) => {
                    {
                        let mut c = cache.lock().await;
                        c.invalidate_with_parent(&path);
                    }
                    {
                        let mut inodes = inodes.lock().await;
                        inodes.remove(&path);
                    }
                    reply.ok();
                }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }

    fn setattr(
        &mut self, _req: &Request, ino: u64, _mode: Option<u32>,
        _uid: Option<u32>, _gid: Option<u32>, size: Option<u64>,
        _atime: Option<TimeOrNow>, _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>, _fh: Option<u64>,
        _crtime: Option<SystemTime>, _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>, _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // For now, just return current attributes (truncate not yet supported)
        let dispatcher = self.dispatcher.clone();
        let cache = self.cache.clone();
        let inodes = self.inodes.clone();

        self.rt.spawn(async move {
            let path = {
                let inodes = inodes.lock().await;
                match inodes.get_path(ino) {
                    Some(p) => p.clone(),
                    None => { reply.error(ENOENT); return; }
                }
            };

            match dispatcher.stat(path.clone()).await {
                Ok(FuseResponse::Stat { file_type, mode, size: file_size, mtime, atime, ctime, .. }) => {
                    let attr = make_file_attr(ino, file_type, mode, file_size, mtime, atime, ctime);
                    {
                        let mut cache = cache.lock().await;
                        cache.put_attr(path, attr);
                    }
                    reply.attr(&TTL, &attr);
                }
                Ok(_) => { reply.error(EIO); }
                Err(e) => { reply.error(Self::err_to_errno(&e)); }
            }
        });
    }
}
