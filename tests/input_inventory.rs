// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
//! Synthetic directories only; no input device or USB access.
#![cfg(target_os = "linux")]
mod common;
use {
    imanlib::evdev::linux::diagnostic_gamepad,
    imanlib::evdev::linux::diagnostic_input_inventory,
    imanlib::evdev::linux::input_inventory,
    imanlib::gamepad::profile::Profile,
};
use imanlib::evdev::DeviceInfo;
use std::collections::BTreeMap;
use std::{fs, path::Path};

fn profile() -> Profile {
    Profile::parse(&common::control_bytes(common::BOARD_EVDEV_FILE)).unwrap()
}

fn node(root: &Path, name: &str, vendor: &str) {
    fs::create_dir_all(root.join("input")).unwrap();
    fs::write(root.join("input").join(name), b"not a device; must never be opened").unwrap();
    let sys = root.join("sys").join(name).join("device");
    fs::create_dir_all(sys.join("id")).unwrap();
    for (field, value) in [("id/bustype", "0003"), ("id/vendor", vendor),
        ("id/product", "5500"), ("name", "ShanWan Twin USB Joystick")] {
        fs::write(sys.join(field), value).unwrap();
    }
}

fn bitmap(codes: &[u16]) -> String {
    let width = usize::BITS as usize;
    let mut words = vec![0usize; codes.iter().copied().max().map_or(1, |code| usize::from(code) / width + 1)];
    for &code in codes { words[usize::from(code) / width] |= 1usize << (usize::from(code) % width); }
    words.into_iter().rev().map(|word| format!("{word:x}")).collect::<Vec<_>>().join(" ")
}

fn capabilities(root: &Path, name: &str, keys: &[u16], axes: &[u16]) {
    let directory = root.join("sys").join(name).join("device/capabilities");
    fs::create_dir(&directory).unwrap();
    for (field, codes) in [("ev", &[0, 1, 3][..]), ("key", keys), ("abs", axes)] {
        fs::write(directory.join(field), bitmap(codes)).unwrap();
    }
}

#[test]
fn only_diagnostic_policy_accepts_unknown_usb_gamepad_capabilities() {
    let root = tempfile::tempdir().unwrap();
    let keys = [304, 305, 307, 308];
    for (index, axes) in [(0, vec![0, 1, 2, 5]), (1, vec![16, 17])] {
        let name = format!("event{index}");
        node(root.path(), &name, "1234");
        capabilities(root.path(), &name, &keys, &axes);
    }
    node(root.path(), "event2", "1234");
    capabilities(root.path(), "event2", &[103, 105, 106, 108], &[0, 1]);
    node(root.path(), "event3", "1234");
    capabilities(root.path(), "event3", &[272, 273, 274, 275], &[0, 1]);
    node(root.path(), "event4", "1234");
    fs::write(root.path().join("sys/event4/device/id/bustype"), "0005").unwrap();
    node(root.path(), "event5", "1234");
    capabilities(root.path(), "event5", &keys, &[0]);
    node(root.path(), "event6", "1234");
    capabilities(root.path(), "event6", &[288, 289, 290, 291], &[0, 1]);

    let normal = input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile()]);
    let diagnostic = diagnostic_input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile()], &[]);

    assert_eq!(normal["eligible_seen"], 0);
    assert_eq!(diagnostic["eligible_seen"], 3);
    for (index, reason) in ["gamepad_capabilities", "gamepad_capabilities", "not_gamepad", "not_gamepad", "non_usb", "not_gamepad", "gamepad_capabilities"]
        .into_iter().enumerate() { assert_eq!(diagnostic["devices"][index]["reason"], reason); }
    let info = DeviceInfo { path: "/synthetic/event0".into(), name: "Synthetic Xbox 360 replica".into(),
        key_codes: keys.to_vec(), axis_codes: vec![0, 1], abs_info: BTreeMap::new(),
        identity: BTreeMap::from([("id/bustype".into(), "0003".into())]) };
    assert!(diagnostic_gamepad(&info));
    let mut changed = info;
    changed.key_codes = vec![103, 105, 106, 108];
    assert!(!diagnostic_gamepad(&changed));
}

#[test]
fn malformed_capability_words_and_ambiguous_profiles_are_not_diagnostic_candidates() {
    let root = tempfile::tempdir().unwrap();
    node(root.path(), "event0", "1234");
    capabilities(root.path(), "event0", &[304, 305, 307, 308], &[0, 1]);
    node(root.path(), "event1", "20bc");
    capabilities(root.path(), "event1", &[304, 305, 307, 308], &[0, 1]);
    for invalid in ["not-hex".into(), "f".repeat(usize::BITS as usize / 4 + 1),
        std::iter::repeat_n("1", 768 / usize::BITS as usize + 1).collect::<Vec<_>>().join(" ")] {
        fs::write(root.path().join("sys/event0/device/capabilities/key"), invalid).unwrap();

        let report = diagnostic_input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile(), profile()], &[]);

        assert_eq!(report["eligible_seen"], 0);
        assert_eq!(report["devices"][0]["reason"], "capabilities_unavailable");
        assert_eq!(report["devices"][1]["reason"], "ambiguous_profiles");
    }
    let conflict = imanlib::controls::Conflict { backend: "evdev".into(), vendor: 0x20bc, product: 0x5500,
        name: "ShanWan Twin USB Joystick".into(), count: 2 };
    let report = diagnostic_input_inventory(&root.path().join("input"), &root.path().join("sys"), &[], &[conflict]);
    assert_eq!(report["devices"][1]["reason"], "ambiguous_profiles");
}

#[test]
fn missing_directory_is_not_reported_as_an_empty_successful_scan() {
    let root = tempfile::tempdir().unwrap();
    let profiles = [profile()];

    let missing = input_inventory(&root.path().join("input"), &root.path().join("sys"), &profiles);
    fs::create_dir(root.path().join("input")).unwrap();
    let empty = input_inventory(&root.path().join("input"), &root.path().join("sys"), &profiles);

    assert!(missing["error"].as_str().unwrap().contains("No such file"));
    assert!(empty["error"].is_null());
    assert_eq!(empty["events_seen"], 0);
    assert_eq!(empty["profiles"], 1);
}

#[test]
fn identities_explain_candidates_rejections_and_unreadable_sysfs_without_opening_nodes() {
    let root = tempfile::tempdir().unwrap();
    node(root.path(), "event0", "20bc");
    node(root.path(), "event1", "1234");
    node(root.path(), "event2", "invalid");
    node(root.path(), "event3", "20bc");
    fs::remove_file(root.path().join("sys/event3/device/name")).unwrap();
    fs::write(root.path().join("input/js0"), b"ignored").unwrap();

    let report = input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile()]);

    assert_eq!(report["events_seen"], 4);
    assert_eq!(report["eligible_seen"], 1);
    for (index, reason) in ["profile_match", "no_matching_profile", "identity_unavailable", "identity_unavailable"]
        .into_iter().enumerate() {
        assert_eq!(report["devices"][index]["reason"], reason);
        assert_eq!(report["devices"][index]["node_type"], "other");
    }
    assert!(report["devices"][2]["error"].as_str().unwrap().contains("id/vendor"));
    assert_eq!(fs::read(root.path().join("input/event0")).unwrap(), b"not a device; must never be opened");
}

#[test]
fn duplicate_profiles_remain_ambiguous_and_oversized_identity_is_not_matched() {
    let root = tempfile::tempdir().unwrap();
    node(root.path(), "event0", "20bc");

    let ambiguous = input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile(), profile()]);
    fs::write(root.path().join("sys/event0/device/name"), "x".repeat(513)).unwrap();
    let oversized = input_inventory(&root.path().join("input"), &root.path().join("sys"), &[profile()]);

    assert_eq!(ambiguous["devices"][0]["reason"], "ambiguous_profiles");
    assert_eq!(ambiguous["devices"][0]["matching_profiles"], 2);
    assert_eq!(ambiguous["eligible_seen"], 0);
    assert_eq!(oversized["devices"][0]["reason"], "identity_unavailable");
    assert!(oversized["devices"][0]["error"].as_str().unwrap().contains("too long"));
}

#[test]
fn inventory_bounds_traversal_and_retained_nodes_with_explicit_omissions() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("input")).unwrap();
    for index in 0..270 {
        fs::write(root.path().join(format!("input/event{index}")), []).unwrap();
    }

    let report = input_inventory(&root.path().join("input"), &root.path().join("sys"), &[]);

    assert_eq!(report["entries_seen"], 256);
    assert_eq!(report["events_seen"], 256);
    assert_eq!(report["devices"].as_array().unwrap().len(), 16);
    assert_eq!(report["devices_omitted"], 240);
    assert_eq!(report["traversal_truncated"], true);
    assert_eq!(report["profiles"], 0);
}
