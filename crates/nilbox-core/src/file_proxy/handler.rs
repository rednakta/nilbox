//! FileProxy — host-side FUSE request handler over VirtualStream.
//!
//! Ported from tauri-app-pipe/src-tauri/src/fuse_proxy/server.rs.
//! Transport changed from Unix socket + tokio_util codec to VirtualStream.

use super::path_manager::{PathManager, PathState};
use super::protocol::*;
use bytes::BytesMut;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tracing::{debug, error, trace};

use crate::vsock::stream::VirtualStream;
use crate::vsock::VsockStream as _;

/// File handle entry
struct OpenFile {
    file: File,
    path: PathBuf,
    is_dir: bool,
}

/// FUSE File Proxy — serves host filesystem requests from VM guest.
pub struct FileProxy {
    path_manager: Arc<RwLock<PathManager>>,
    open_files: Arc<Mutex<HashMap<u64, OpenFile>>>,
    next_fd: AtomicU64,
    read_only: bool,
    vm_mount: String,
    shutdown_notify: Arc<Notify>,
}

impl FileProxy {
    pub fn new(shared_path: PathBuf, read_only: bool, vm_mount: String) -> Self {
        Self {
            path_manager: Arc::new(RwLock::new(PathManager::new(shared_path))),
            open_files: Arc::new(Mutex::new(HashMap::new())),
            next_fd: AtomicU64::new(1),
            read_only,
            vm_mount,
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    /// 셧다운 신호 — listen() 루프를 종료한다.
    pub fn shutdown(&self) {
        self.shutdown_notify.notify_one();
    }

    /// 열린 파일 수를 반환하고, 1개 이상이면 언마운트 대기 oneshot을 설정한다.
    /// Returns (pending_handles, Option<oneshot_rx>).
    /// pending_handles == 0: 즉시 언마운트 가능; > 0: rx로 대기.
    pub async fn request_unmount(&self) -> (usize, Option<tokio::sync::oneshot::Receiver<()>>) {
        let mut pm = self.path_manager.write().await;
        let count = pm.open_handles_count();
        if count == 0 {
            (0, None)
        } else {
            let rx = pm.set_unmount_pending();
            (count, Some(rx))
        }
    }

    /// 강제 언마운트 — 열린 파일을 무시하고 즉시 종료
    pub async fn force_unmount(&self) {
        {
            let mut files = self.open_files.lock().await;
            files.clear();
        }
        {
            let mut pm = self.path_manager.write().await;
            pm.clear_open_handles();
        }
        self.shutdown_notify.notify_one();
    }

    pub fn path_manager(&self) -> Arc<RwLock<PathManager>> {
        self.path_manager.clone()
    }

    /// Listen on a VirtualStream for FUSE requests.
    /// Runs until the stream closes.
    pub async fn listen(&self, mut stream: VirtualStream) {
        debug!("FileProxy: listening on stream {}", stream.stream_id);

        // Send mount point handshake: u16_le(len) + path bytes
        // vm-agent reads this before starting fuser::mount2
        {
            let path_bytes = self.vm_mount.as_bytes();
            let len = path_bytes.len() as u16;
            let mut handshake = Vec::with_capacity(2 + path_bytes.len());
            handshake.extend_from_slice(&len.to_le_bytes());
            handshake.extend_from_slice(path_bytes);
            if let Err(e) = stream.writer().write(&handshake).await {
                error!("FileProxy: failed to send mount point handshake: {}", e);
                return;
            }
            debug!("FileProxy: sent mount point handshake: {}", self.vm_mount);
        }

        // Response channel for serialized writes
        let (tx, mut rx) = mpsc::channel::<FuseResponse>(100);

        // Set notification channel in path manager
        {
            let mut pm = self.path_manager.write().await;
            pm.set_notification_channel(tx.clone());
        }

        // Writer task: encode responses and write to stream
        let writer = stream.writer();
        tokio::spawn(async move {
            while let Some(response) = rx.recv().await {
                let encoded = encode_response(&response);
                if let Err(e) = writer.write(&encoded).await {
                    error!("FileProxy: write error: {}", e);
                    break;
                }
            }
        });

        // Request processing loop
        let shutdown = self.shutdown_notify.clone();
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            tokio::select! {
                result = stream.read() => {
                    match result {
                        Ok(data) => {
                            buf.extend_from_slice(&data);

                            // Process all complete requests in buffer
                            loop {
                                match decode_request(&mut buf) {
                                    Ok(Some(request)) => {
                                        trace!("FileProxy: request {:?}", request);
                                        let response = self.handle_request(request).await;
                                        if let Err(e) = tx.send(response).await {
                                            error!("FileProxy: failed to queue response: {}", e);
                                            return;
                                        }
                                    }
                                    Ok(None) => break, // need more data
                                    Err(e) => {
                                        error!("FileProxy: decode error: {}", e);
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug!("FileProxy: stream closed: {}", e);
                            break;
                        }
                    }
                }
                _ = shutdown.notified() => {
                    debug!("FileProxy: shutdown requested");
                    break;
                }
            }
        }

        debug!("FileProxy: disconnected");
    }

    async fn handle_request(&self, request: FuseRequest) -> FuseResponse {
        match request {
            FuseRequest::Open { request_id, flags, mode, path } => {
                self.handle_open(request_id, flags, mode, &path).await
            }
            FuseRequest::Read { request_id, fd, offset, size } => {
                self.handle_read(request_id, fd, offset, size).await
            }
            FuseRequest::Write { request_id, fd, offset, data } => {
                self.handle_write(request_id, fd, offset, data).await
            }
            FuseRequest::Close { request_id, fd } => {
                self.handle_close(request_id, fd).await
            }
            FuseRequest::Readdir { request_id, fd, offset, max_entries } => {
                self.handle_readdir(request_id, fd, offset, max_entries).await
            }
            FuseRequest::Stat { request_id, path } => {
                self.handle_stat(request_id, &path).await
            }
            FuseRequest::Mkdir { request_id, mode, path } => {
                self.handle_mkdir(request_id, mode, &path).await
            }
            FuseRequest::Remove { request_id, is_dir, path } => {
                self.handle_remove(request_id, is_dir, &path).await
            }
            FuseRequest::PathQuery { request_id } => {
                self.handle_path_query(request_id).await
            }
            FuseRequest::Ping { request_id } => {
                FuseResponse::Pong { request_id }
            }
        }
    }

    async fn handle_open(&self, request_id: RequestId, flags: u32, _mode: u32, path: &str) -> FuseResponse {
        let pm = self.path_manager.read().await;

        if pm.state() != PathState::Active {
            return FuseResponse::Open { request_id, status: StatusCode::ErrBusy, fd: 0 };
        }

        // Check write access
        let access_mode = flags & 0x0003;
        let is_write = access_mode == 1 || access_mode == 2; // O_WRONLY or O_RDWR
        let create = (flags & 0x40) != 0; // O_CREAT
        if self.read_only && (is_write || create) {
            return FuseResponse::Open { request_id, status: StatusCode::ErrAccess, fd: 0 };
        }

        let full_path = if create {
            match pm.validate_new_path(path) {
                Ok(p) => p,
                Err(_) => return FuseResponse::Open { request_id, status: StatusCode::ErrSandboxed, fd: 0 },
            }
        } else {
            match pm.resolve_path(path) {
                Ok(p) => p,
                Err(_) => return FuseResponse::Open { request_id, status: StatusCode::ErrNoent, fd: 0 },
            }
        };
        drop(pm);

        let is_dir = full_path.is_dir();

        let file = if is_dir {
            #[cfg(unix)]
            let result = File::open(&full_path);
            #[cfg(target_os = "windows")]
            let result = {
                use std::os::windows::fs::OpenOptionsExt;
                // Windows requires FILE_FLAG_BACKUP_SEMANTICS to open directories
                OpenOptions::new()
                    .read(true)
                    .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS
                    .open(&full_path)
            };
            match result {
                Ok(f) => f,
                Err(e) => return FuseResponse::Open { request_id, status: StatusCode::from(e.kind()), fd: 0 },
            }
        } else {
            let read = access_mode == 0 || access_mode == 2;
            let write = access_mode == 1 || access_mode == 2;
            let create_flag = (flags & 0x40) != 0;
            let truncate = (flags & 0x200) != 0;
            let append = (flags & 0x400) != 0;

            let result = OpenOptions::new()
                .read(read)
                .write(write)
                .create(create_flag)
                .truncate(truncate)
                .append(append)
                .open(&full_path);

            match result {
                Ok(f) => f,
                Err(e) => return FuseResponse::Open { request_id, status: StatusCode::from(e.kind()), fd: 0 },
            }
        };

        let fd = self.next_fd.fetch_add(1, Ordering::SeqCst);

        {
            let mut pm = self.path_manager.write().await;
            pm.register_handle(fd);
        }

        {
            let mut files = self.open_files.lock().await;
            files.insert(fd, OpenFile { file, path: full_path, is_dir });
        }

        FuseResponse::Open { request_id, status: StatusCode::Ok, fd }
    }

    async fn handle_read(&self, request_id: RequestId, fd: u64, offset: u64, size: u32) -> FuseResponse {
        let mut files = self.open_files.lock().await;
        let open_file = match files.get_mut(&fd) {
            Some(f) => f,
            None => return FuseResponse::Read { request_id, status: StatusCode::ErrNoent, data: BytesMut::new() },
        };

        if let Err(e) = open_file.file.seek(SeekFrom::Start(offset)) {
            return FuseResponse::Read { request_id, status: StatusCode::from(e.kind()), data: BytesMut::new() };
        }

        let mut buffer = vec![0u8; size as usize];
        match open_file.file.read(&mut buffer) {
            Ok(n) => {
                buffer.truncate(n);
                FuseResponse::Read { request_id, status: StatusCode::Ok, data: BytesMut::from(&buffer[..]) }
            }
            Err(e) => FuseResponse::Read { request_id, status: StatusCode::from(e.kind()), data: BytesMut::new() },
        }
    }

    async fn handle_write(&self, request_id: RequestId, fd: u64, offset: u64, data: BytesMut) -> FuseResponse {
        if self.read_only {
            return FuseResponse::Write { request_id, status: StatusCode::ErrAccess, written: 0 };
        }

        let mut files = self.open_files.lock().await;
        let open_file = match files.get_mut(&fd) {
            Some(f) => f,
            None => return FuseResponse::Write { request_id, status: StatusCode::ErrNoent, written: 0 },
        };

        if let Err(e) = open_file.file.seek(SeekFrom::Start(offset)) {
            return FuseResponse::Write { request_id, status: StatusCode::from(e.kind()), written: 0 };
        }

        match open_file.file.write(&data) {
            Ok(n) => FuseResponse::Write { request_id, status: StatusCode::Ok, written: n as u32 },
            Err(e) => FuseResponse::Write { request_id, status: StatusCode::from(e.kind()), written: 0 },
        }
    }

    async fn handle_close(&self, request_id: RequestId, fd: u64) -> FuseResponse {
        {
            let mut files = self.open_files.lock().await;
            files.remove(&fd);
        }

        {
            let mut pm = self.path_manager.write().await;
            pm.unregister_handle(fd);
        }

        FuseResponse::Close { request_id, status: StatusCode::Ok }
    }

    async fn handle_readdir(&self, request_id: RequestId, fd: u64, _offset: u64, max_entries: u32) -> FuseResponse {
        let files = self.open_files.lock().await;
        let open_file = match files.get(&fd) {
            Some(f) => f,
            None => return FuseResponse::Readdir { request_id, status: StatusCode::ErrNoent, entries: vec![] },
        };

        if !open_file.is_dir {
            return FuseResponse::Readdir { request_id, status: StatusCode::ErrNotdir, entries: vec![] };
        }

        let dir_path = open_file.path.clone();
        drop(files);

        match fs::read_dir(&dir_path) {
            Ok(entries) => {
                let mut result = Vec::new();
                for entry in entries.take(max_entries as usize) {
                    if let Ok(entry) = entry {
                        let file_type = if entry.path().is_dir() {
                            FileType::Directory
                        } else if entry.path().is_symlink() {
                            FileType::Symlink
                        } else {
                            FileType::Regular
                        };
                        let raw_name = entry.file_name().to_string_lossy().to_string();
                        #[cfg(target_os = "macos")]
                        let name = {
                            use unicode_normalization::UnicodeNormalization;
                            raw_name.nfc().collect::<String>()
                        };
                        #[cfg(not(target_os = "macos"))]
                        let name = raw_name;
                        result.push(DirEntry {
                            file_type,
                            name,
                        });
                    }
                }
                FuseResponse::Readdir { request_id, status: StatusCode::Ok, entries: result }
            }
            Err(e) => FuseResponse::Readdir { request_id, status: StatusCode::from(e.kind()), entries: vec![] },
        }
    }

    async fn handle_stat(&self, request_id: RequestId, path: &str) -> FuseResponse {
        let pm = self.path_manager.read().await;
        let full_path = match pm.resolve_path(path) {
            Ok(p) => p,
            Err(_) => return FuseResponse::Stat { request_id, status: StatusCode::ErrNoent, attr: FileAttr::default() },
        };
        drop(pm);

        match fs::metadata(&full_path) {
            Ok(meta) => {
                #[cfg(unix)]
                let (mode, mtime, atime, ctime) = {
                    use std::os::unix::fs::MetadataExt;
                    (meta.mode(), meta.mtime() as u64, meta.atime() as u64, meta.ctime() as u64)
                };
                #[cfg(target_os = "windows")]
                let (mode, mtime, atime, ctime) = {
                    let readonly = meta.permissions().readonly();
                    let mode = if meta.is_dir() {
                        if readonly { 0o555u32 } else { 0o755u32 }
                    } else {
                        if readonly { 0o444u32 } else { 0o644u32 }
                    };
                    let to_epoch = |t: std::io::Result<std::time::SystemTime>| -> u64 {
                        t.ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0)
                    };
                    (mode, to_epoch(meta.modified()), to_epoch(meta.accessed()), to_epoch(meta.created()))
                };

                let file_type = if meta.is_dir() {
                    FileType::Directory as u8
                } else if meta.file_type().is_symlink() {
                    FileType::Symlink as u8
                } else {
                    FileType::Regular as u8
                };

                FuseResponse::Stat {
                    request_id,
                    status: StatusCode::Ok,
                    attr: FileAttr {
                        file_type,
                        mode,
                        size: meta.len(),
                        mtime,
                        atime,
                        ctime,
                    },
                }
            }
            Err(e) => FuseResponse::Stat { request_id, status: StatusCode::from(e.kind()), attr: FileAttr::default() },
        }
    }

    async fn handle_mkdir(&self, request_id: RequestId, mode: u32, path: &str) -> FuseResponse {
        if self.read_only {
            return FuseResponse::Mkdir { request_id, status: StatusCode::ErrAccess };
        }

        let pm = self.path_manager.read().await;
        let full_path = match pm.validate_new_path(path) {
            Ok(p) => p,
            Err(_) => return FuseResponse::Mkdir { request_id, status: StatusCode::ErrSandboxed },
        };
        drop(pm);

        match fs::create_dir(&full_path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&full_path, fs::Permissions::from_mode(mode));
                }
                FuseResponse::Mkdir { request_id, status: StatusCode::Ok }
            }
            Err(e) => FuseResponse::Mkdir { request_id, status: StatusCode::from(e.kind()) },
        }
    }

    async fn handle_remove(&self, request_id: RequestId, is_dir: bool, path: &str) -> FuseResponse {
        if self.read_only {
            return FuseResponse::Remove { request_id, status: StatusCode::ErrAccess };
        }

        let pm = self.path_manager.read().await;
        let full_path = match pm.resolve_path(path) {
            Ok(p) => p,
            Err(_) => return FuseResponse::Remove { request_id, status: StatusCode::ErrNoent },
        };
        drop(pm);

        let result = if is_dir { fs::remove_dir(&full_path) } else { fs::remove_file(&full_path) };

        match result {
            Ok(()) => FuseResponse::Remove { request_id, status: StatusCode::Ok },
            Err(e) => FuseResponse::Remove { request_id, status: StatusCode::from(e.kind()) },
        }
    }

    async fn handle_path_query(&self, request_id: RequestId) -> FuseResponse {
        let pm = self.path_manager.read().await;
        FuseResponse::PathQuery {
            request_id,
            status: StatusCode::Ok,
            state: pm.state() as u8,
            path: pm.current_path().to_string_lossy().to_string(),
        }
    }
}
