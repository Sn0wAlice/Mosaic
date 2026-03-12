#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Mosaic only supports Linux and macOS");

pub mod fs;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FuseError {
    #[error("macFUSE is not installed. Install it with: brew install macfuse")]
    MacFuseNotInstalled,
    #[error("FUSE mount failed: {0}")]
    MountFailed(String),
    #[error("Unmount failed: {0}")]
    UnmountFailed(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Checks for macFUSE on macOS.
#[cfg(target_os = "macos")]
pub fn check_fuse() -> Result<(), FuseError> {
    if !std::path::Path::new("/Library/Filesystems/macfuse.fs").exists() {
        return Err(FuseError::MacFuseNotInstalled);
    }
    Ok(())
}

/// On Linux, FUSE is typically available via libfuse3.
#[cfg(target_os = "linux")]
pub fn check_fuse() -> Result<(), FuseError> {
    if !std::path::Path::new("/dev/fuse").exists() {
        return Err(FuseError::MountFailed(
            "FUSE device not found. Install fuse3: sudo apt install fuse3 libfuse3-dev".into(),
        ));
    }
    Ok(())
}
