// SPDX-License-Identifier: BSD-3-Clause
//! Only regular files and malformed paths: never opens real DRM nodes.
#![cfg(all(feature = "drm", target_os = "linux"))]
use imanlib::kms::LinuxDrm;
use std::{fs, os::unix::fs::symlink};

#[test]
fn regular_files_and_symlinks_are_refused_before_any_ioctl_or_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card2");
    fs::write(&path, b"unchanged fixture").unwrap();

    let error = LinuxDrm::open(&path, None).err().unwrap();

    assert!(error.to_string().contains("unexpected DRM primary device"));
    assert_eq!(fs::read(&path).unwrap(), b"unchanged fixture");
    let link = dir.path().join("card0");
    symlink(&path, &link).unwrap();
    assert!(LinuxDrm::open(&link, None).is_err());
}

#[test]
fn invalid_primary_node_paths_are_rejected_without_device_discovery() {
    for path in [
        "card0",
        "/fixture/renderD128",
        "/fixture/card64",
        "/fixture/card-1",
    ] {
        assert!(LinuxDrm::open(path.as_ref(), None).is_err());
    }
}
