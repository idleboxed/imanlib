// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
mod common;
use common::{BOARD_EVDEV_FILE, HID_FILE, PC_EVDEV_FILE};
use imanlib::controls;
use std::{fs, path::Path};

fn write_controls(directory: &Path) {
    for name in [HID_FILE, BOARD_EVDEV_FILE, PC_EVDEV_FILE] {
        fs::write(directory.join(name), common::control_bytes(name)).unwrap();
    }
}

fn synthetic_profile(name: &str, index: usize) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(&common::control_bytes(name)).unwrap();
    value["name"] = format!("Synthetic USB pad {index}").into();
    value["match"]["vendor"] = "1234".into();
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn defaults_are_read_once_from_the_requested_directory() {
    let directory = tempfile::tempdir().unwrap();
    write_controls(directory.path());
    let before: Vec<_> = [HID_FILE, BOARD_EVDEV_FILE, PC_EVDEV_FILE].into_iter()
        .map(|name| fs::metadata(directory.path().join(name)).unwrap().modified().unwrap()).collect();

    let profiles = controls::load(directory.path(), None, None).unwrap();

    assert_eq!(profiles.hid.len(), 1);
    assert!(profiles.hid[0].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
    assert_eq!(profiles.evdev.len(), 2);
    assert!(profiles.evdev[0].matches(3, 0x20bc, 0x5500, "ShanWan Twin USB Joystick"));
    assert!(profiles.evdev[1].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
    for (name, modified) in [HID_FILE, BOARD_EVDEV_FILE, PC_EVDEV_FILE].into_iter().zip(before) {
        assert_eq!(fs::metadata(directory.path().join(name)).unwrap().modified().unwrap(), modified);
    }
    let board = directory.path().join(BOARD_EVDEV_FILE);
    fs::write(&board, synthetic_profile(BOARD_EVDEV_FILE, 0)).unwrap();

    let updated = controls::load(directory.path(), None, None).unwrap();

    assert!(updated.evdev[0].matches(3, 0x1234, 0x5500, "Synthetic USB pad 0"));
    assert!(profiles.evdev[0].matches(3, 0x20bc, 0x5500, "ShanWan Twin USB Joystick"));
}

#[test]
fn arbitrary_filenames_and_identities_need_no_index_or_both_backends() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("z.json"), synthetic_profile(HID_FILE, 2)).unwrap();
    fs::write(directory.path().join("a.json"), synthetic_profile(HID_FILE, 1)).unwrap();
    fs::write(directory.path().join("README.txt"), b"not a profile").unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested/bad.json"), b"invalid").unwrap();

    let profiles = controls::load(directory.path(), None, None).unwrap();

    assert_eq!(profiles.hid.len(), 2);
    assert!(profiles.evdev.is_empty());
    assert!(profiles.hid[0].matches(3, 0x1234, 0x0555, "Synthetic USB pad 1"));
    assert!(profiles.hid[1].matches(3, 0x1234, 0x0555, "Synthetic USB pad 2"));
    assert!(controls::load_probe(directory.path()).is_err());
}

#[test]
fn empty_or_missing_directory_is_not_silently_ignored() {
    let directory = tempfile::tempdir().unwrap();
    assert!(controls::load(directory.path(), None, None).err().unwrap().to_string().contains("no input profiles"));
    assert_eq!(controls::load(&directory.path().join("absent"), None, None).err().unwrap().kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn explicit_files_replace_only_their_backend_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let overrides = tempfile::tempdir().unwrap();
    write_controls(directory.path());
    let hid = overrides.path().join("custom-hid.json");
    let evdev = overrides.path().join("custom-evdev.json");
    fs::write(&hid, synthetic_profile(HID_FILE, 0)).unwrap();
    fs::write(&evdev, synthetic_profile(PC_EVDEV_FILE, 0)).unwrap();

    let profiles = controls::load(&directory.path().join("absent"), Some(&hid), Some(&evdev)).unwrap();
    assert_eq!(profiles.hid.len(), 1);
    assert_eq!(profiles.evdev.len(), 1);
    assert!(profiles.evdev[0].matches(3, 0x1234, 0x0555, "Synthetic USB pad 0"));
    let profiles = controls::load(directory.path(), None, Some(&evdev)).unwrap();
    assert!(profiles.hid[0].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
    assert_eq!(profiles.evdev.len(), 1);
    let profiles = controls::load(directory.path(), Some(&hid), None).unwrap();
    assert!(profiles.hid[0].matches(3, 0x1234, 0x0555, "Synthetic USB pad 0"));
    assert_eq!(profiles.evdev.len(), 2);
}

#[test]
fn duplicate_matches_disable_only_the_ambiguous_backend_and_keep_other_profiles() {
    for name in [HID_FILE, PC_EVDEV_FILE] {
        let directory = tempfile::tempdir().unwrap();
        write_controls(directory.path());
        assert!(controls::load(directory.path(), None, None).unwrap().conflicts.is_empty());
        fs::write(directory.path().join("duplicate.json"), common::control_bytes(name)).unwrap();

        let profiles = controls::load(directory.path(), None, None).unwrap();

        assert_eq!(profiles.conflicts.len(), 1);
        let conflict = &profiles.conflicts[0];
        assert_eq!(conflict.count, 2);
        assert_eq!((conflict.vendor, conflict.product), (0x2563, 0x0555));
        assert_eq!(conflict.name, "ShanWan Twin USB Joystick");
        assert!(profiles.evdev[0].matches(3, 0x20bc, 0x5500, "ShanWan Twin USB Joystick"));
        if name == HID_FILE {
            assert_eq!(conflict.backend, "hidraw");
            assert!(profiles.hid.is_empty());
            assert_eq!(profiles.evdev.len(), 2);
            assert!(profiles.evdev[1].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
        } else {
            assert_eq!(conflict.backend, "evdev");
            assert_eq!(profiles.evdev.len(), 1);
            assert_eq!(profiles.hid.len(), 1);
            assert!(profiles.hid[0].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
        }
    }
}

#[test]
fn all_profiles_can_be_conflicting_without_failing_normal_or_probe_loading() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["a.json", "b.json"] {
        fs::write(directory.path().join(name), common::control_bytes(BOARD_EVDEV_FILE)).unwrap();
    }

    for profiles in [controls::load(directory.path(), None, None).unwrap(), controls::load_probe(directory.path()).unwrap()] {
        assert!(profiles.hid.is_empty());
        assert!(profiles.evdev.is_empty());
        assert_eq!(profiles.conflicts.len(), 1);
        let conflict = &profiles.conflicts[0];
        assert_eq!(conflict.to_string(), "для 20bc:5500 найдено 2 профиля (evdev, \"ShanWan Twin USB Joystick\"); этот backend отключён");
    }
}

#[test]
fn conflict_diagnostics_are_deterministic_and_count_every_profile() {
    let directory = tempfile::tempdir().unwrap();
    write_controls(directory.path());
    for index in 0..4 {
        fs::write(directory.path().join(format!("{index}.json")), common::control_bytes(BOARD_EVDEV_FILE)).unwrap();
    }
    let original = controls::load(directory.path(), None, None).unwrap();
    fs::rename(directory.path().join("0.json"), directory.path().join("z.json")).unwrap();

    let renamed = controls::load(directory.path(), None, None).unwrap();

    assert_eq!(original.conflicts, renamed.conflicts);
    assert_eq!(original.conflicts[0].count, 5);
    assert!(original.conflicts[0].to_string().contains("найдено 5 профилей"));
    assert_eq!(renamed.evdev.len(), 1);
    assert_eq!(renamed.hid.len(), 1);
    let explicit = directory.path().join(BOARD_EVDEV_FILE);
    let overridden = controls::load(directory.path(), None, Some(&explicit)).unwrap();
    assert!(overridden.conflicts.is_empty());
    assert_eq!(overridden.evdev.len(), 1);
}

#[test]
fn invalid_unknown_backend_and_oversized_profiles_report_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut unknown: serde_json::Value = serde_json::from_slice(&common::control_bytes(HID_FILE)).unwrap();
    unknown["backend"] = "unknown".into();
    for name in [HID_FILE, BOARD_EVDEV_FILE, PC_EVDEV_FILE] {
        for bytes in [b"{}".to_vec(), vec![b' '; 32769], serde_json::to_vec(&unknown).unwrap()] {
            write_controls(directory.path());
            let path = directory.path().join(name);
            fs::write(&path, bytes).unwrap();

            let error = controls::load(directory.path(), None, None).err().unwrap();

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains(path.to_str().unwrap()), "{error}");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn symlinks_and_nonregular_profiles_remain_rejected() {
    let directory = tempfile::tempdir().unwrap();
    write_controls(directory.path());
    let original = directory.path().join(HID_FILE);
    let other = directory.path().join("other.txt");
    fs::rename(&original, &other).unwrap();
    std::os::unix::fs::symlink(&other, &original).unwrap();
    assert!(controls::load(directory.path(), None, None).is_err());
    fs::remove_file(&original).unwrap();
    fs::create_dir(&original).unwrap();
    assert!(controls::load(directory.path(), None, None).is_err());
    assert_eq!(fs::read(other).unwrap(), common::control_bytes(HID_FILE));
}

#[test]
fn profile_count_total_bytes_and_directory_entries_are_bounded() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..64 {
        fs::write(directory.path().join(format!("{index:02}.json")), synthetic_profile(HID_FILE, index)).unwrap();
    }
    assert_eq!(controls::load(directory.path(), None, None).unwrap().hid.len(), 64);
    fs::write(directory.path().join("extra.json"), synthetic_profile(HID_FILE, 64)).unwrap();
    assert!(controls::load(directory.path(), None, None).err().unwrap().to_string().contains("64 JSON"));

    let directory = tempfile::tempdir().unwrap();
    for index in 0..16 {
        let mut bytes = synthetic_profile(HID_FILE, index);
        bytes.resize(32 * 1024, b' ');
        fs::write(directory.path().join(format!("{index:02}.json")), bytes).unwrap();
    }
    assert_eq!(controls::load(directory.path(), None, None).unwrap().hid.len(), 16);
    fs::write(directory.path().join("extra.json"), synthetic_profile(HID_FILE, 16)).unwrap();
    assert!(controls::load(directory.path(), None, None).err().unwrap().to_string().contains("512 KiB"));

    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("pad.json"), common::control_bytes(HID_FILE)).unwrap();
    for index in 0..255 {
        fs::write(directory.path().join(format!("{index}.txt")), b"").unwrap();
    }
    assert!(controls::load(directory.path(), None, None).is_ok());
    fs::write(directory.path().join("extra.txt"), b"").unwrap();
    assert!(controls::load(directory.path(), None, None).err().unwrap().to_string().contains("256 entries"));
}

#[test]
fn raw_probe_uses_all_declared_evdev_profiles() {
    let directory = tempfile::tempdir().unwrap();
    write_controls(directory.path());
    let profiles = controls::load_probe(directory.path()).unwrap();
    assert_eq!(profiles.evdev.len(), 2);
    assert!(profiles.evdev[0].matches(3, 0x20bc, 0x5500, "ShanWan Twin USB Joystick"));
    assert!(profiles.evdev[1].matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
}

#[test]
fn incomplete_optional_pairs_or_no_navigation_are_invalid_profiles() {
    use imanlib::gamepad::profile::Profile;
    for axes in [serde_json::json!({"left_stick_x":0}), serde_json::json!({"dpad_y":17}), serde_json::json!({})] {
        let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
        document["axes"] = axes;
        for key in ["up", "down", "left", "right"] { document["buttons"].as_object_mut().unwrap().remove(key); }

        assert!(Profile::parse(&serde_json::to_vec(&document).unwrap()).is_err());
    }
}

#[test]
fn evdev_profile_validation_rejects_typos_and_inconsistent_bindings() {
    use imanlib::gamepad::profile::Profile;
    for (section, key, value) in [
        ("buttons", "west", serde_json::json!(768)),
        ("buttons", "west", serde_json::json!(21)),
        ("buttons", "unknown", serde_json::json!(22)),
        ("axes", "left_stick_x", serde_json::json!(16)),
        ("axes", "left_stick_x", serde_json::json!(64)),
        ("match", "vendor", serde_json::json!("nope")),
    ] {
        let mut directory_item: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
        directory_item[section][key] = value;
        assert!(Profile::parse(&serde_json::to_vec(&directory_item).unwrap()).is_err());
    }
    let duplicate = String::from_utf8(common::control_bytes(common::BOARD_EVDEV_FILE))
        .unwrap()
        .replace("\"west\": 45", "\"west\": 45, \"west\": 46");
    assert!(Profile::parse(duplicate.as_bytes()).is_err());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evdev.json");
    std::fs::write(&file, &common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    assert!(Profile::load(&file)
        .unwrap()
        .matches(3, 0x20bc, 0x5500, "ShanWan Twin USB Joystick"));
}

#[test]
fn evdev_profile_ignores_unknown_fields_and_requires_known_fields() {
    use imanlib::gamepad::profile::Profile;
    let mut document: serde_json::Value = serde_json::from_slice(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap();
    document["version"] = serde_json::json!(999);
    document["future"] = serde_json::json!(true);
    document["match"]["future"] = serde_json::json!(true);
    document["axes"]["future"] = serde_json::json!(true);
    assert!(Profile::parse(&serde_json::to_vec(&document).unwrap()).is_ok());

    document.as_object_mut().unwrap().remove("backend");
    let error = Profile::parse(&serde_json::to_vec(&document).unwrap())
        .err()
        .unwrap();
    assert!(error.to_string().contains("missing field `backend`"));
}
