//! Process-level security hardening: memory locking, core dump prevention,
//! ptrace protection, and other OS-level defenses.

use tracing::{info, warn};

/// Applies all available process-level security hardening.
/// Should be called early in main(), before any keys are derived.
/// Non-fatal: logs warnings if a measure fails but does not abort.
pub fn harden_process() {
    disable_core_dumps();
    #[cfg(target_os = "linux")]
    disable_ptrace();
    info!("Process security hardening applied");
}

/// Sets RLIMIT_CORE to 0 to prevent core dumps that could leak key material.
fn disable_core_dumps() {
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let ret = libc::setrlimit(libc::RLIMIT_CORE, &limit);
        if ret == 0 {
            info!("Core dumps disabled (RLIMIT_CORE = 0)");
        } else {
            warn!("Failed to disable core dumps (setrlimit returned {})", ret);
        }
    }
}

/// On Linux, prevents other processes from attaching a debugger (ptrace).
/// This blocks tools like gdb/strace from reading process memory.
#[cfg(target_os = "linux")]
fn disable_ptrace() {
    unsafe {
        // PR_SET_DUMPABLE = 3, arg = 0 means not dumpable
        let ret = libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        if ret == 0 {
            info!("ptrace protection enabled (PR_SET_DUMPABLE = 0)");
        } else {
            warn!("Failed to set PR_SET_DUMPABLE: errno {}", *libc::__errno_location());
        }
    }
}

/// Locks a memory region so it cannot be swapped to disk.
/// Call this on buffers containing encryption keys.
/// Returns true if successful.
pub fn mlock(ptr: *const u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    unsafe { libc::mlock(ptr as *const libc::c_void, len) == 0 }
}

/// Unlocks a previously mlocked memory region.
pub fn munlock(ptr: *const u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    unsafe { libc::munlock(ptr as *const libc::c_void, len) == 0 }
}

/// Locks a 32-byte key array into physical memory.
/// Logs a warning if mlock fails (e.g., insufficient RLIMIT_MEMLOCK).
pub fn mlock_key(key: &[u8; 32]) {
    if !mlock(key.as_ptr(), 32) {
        warn!(
            "Failed to mlock key memory — keys may be swapped to disk. \
             Consider raising RLIMIT_MEMLOCK."
        );
    }
}

/// Unlocks a 32-byte key from physical memory pinning.
pub fn munlock_key(key: &[u8; 32]) {
    munlock(key.as_ptr(), 32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mlock_munlock_key() {
        let key = [0xABu8; 32];
        // mlock may fail on CI without sufficient RLIMIT_MEMLOCK,
        // so we just verify it doesn't crash.
        mlock_key(&key);
        munlock_key(&key);
    }

    #[test]
    fn test_mlock_empty() {
        assert!(mlock(std::ptr::null(), 0));
        assert!(munlock(std::ptr::null(), 0));
    }

    #[test]
    fn test_disable_core_dumps() {
        // Should not panic
        disable_core_dumps();

        // Verify RLIMIT_CORE is 0
        unsafe {
            let mut limit = libc::rlimit {
                rlim_cur: 999,
                rlim_max: 999,
            };
            libc::getrlimit(libc::RLIMIT_CORE, &mut limit);
            assert_eq!(limit.rlim_cur, 0);
            assert_eq!(limit.rlim_max, 0);
        }
    }
}
