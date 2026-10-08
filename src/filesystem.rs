// SPDX-License-Identifier: BSD-3-Clause
//! Explicit Linux filesystem queries through held descriptors; no file creation.
use std::{fs::File, io, os::fd::AsRawFd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilesystemInfo {
    pub is_tmpfs: bool,
    /// Space available to the caller, not total capacity or reserved free blocks.
    pub available_bytes: u64,
}

/// Query the held file, without resolving its path or defining storage policy.
pub fn read_filesystem(file: &File) -> io::Result<FilesystemInfo> {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: borrowed open descriptor and writable storage; inspect only on success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(FilesystemInfo {
        is_tmpfs: info.f_type as u64 == libc::TMPFS_MAGIC as u64,
        available_bytes: (info.f_bsize as u64).saturating_mul(info.f_bavail),
    })
}
