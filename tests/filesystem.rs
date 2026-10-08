// SPDX-License-Identifier: BSD-3-Clause
//! Queries of isolated RAM files and procfs; no mounts or block devices.
use imanlib::filesystem::read_filesystem;
use std::{fs::File, io::{Seek, Write}, os::unix::fs::symlink};

#[test]
fn filesystem_query_reports_available_tmpfs_space_without_changing_the_file() {
    let mut file = tempfile::NamedTempFile::new_in("/dev/shm").unwrap();
    file.write_all(b"synthetic RAM data").unwrap();
    let position = file.as_file_mut().stream_position().unwrap();
    let length = file.as_file().metadata().unwrap().len();

    let info = read_filesystem(file.as_file()).unwrap();

    assert!(info.is_tmpfs);
    assert!(info.available_bytes > 0);
    assert_eq!(file.as_file_mut().stream_position().unwrap(), position);
    assert_eq!(file.as_file().metadata().unwrap().len(), length);
}

#[test]
fn filesystem_query_does_not_classify_procfs_as_tmpfs() {
    let directory = File::open("/proc").unwrap();

    let info = read_filesystem(&directory).unwrap();

    assert!(!info.is_tmpfs);
    assert_eq!(info.available_bytes, 0);
}

#[test]
fn filesystem_query_uses_the_held_directory_after_its_path_is_replaced() {
    let root = tempfile::tempdir_in("/dev/shm").unwrap();
    let path = root.path().join("runtime");
    std::fs::create_dir(&path).unwrap();
    let directory = File::open(&path).unwrap();
    std::fs::rename(&path, root.path().join("held")).unwrap();
    symlink("/proc", &path).unwrap();

    let held = read_filesystem(&directory).unwrap();
    let replacement = read_filesystem(&File::open(&path).unwrap()).unwrap();

    assert!(held.is_tmpfs);
    assert!(!replacement.is_tmpfs);
}
