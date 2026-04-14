//! FUSE file proxy — host-side shared directory access via VSOCK.

pub mod protocol;
pub mod path_manager;
pub mod handler;

pub use handler::FileProxy;
pub use path_manager::{PathManager, PathState};
