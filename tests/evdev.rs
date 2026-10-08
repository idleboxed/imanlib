// SPDX-License-Identifier: BSD-3-Clause
//! Synthetic capabilities and input packets, not physical controller measurements.
use imanlib::evdev::{find_retroarch_button_index, DeviceInfo, Event};
use serde_json::json;
use std::collections::BTreeMap;

fn create_device_info() -> DeviceInfo {
    DeviceInfo {
        path: "/synthetic/event0".into(),
        name: "Synthetic pad".into(),
        axis_codes: vec![0, 1],
        key_codes: vec![28, 103, 256],
        abs_info: BTreeMap::from([(0, [128, 0, 255, 0, 15, 0])]),
        identity: BTreeMap::from([("id/bustype".into(), "0003".into())]),
    }
}

#[test]
fn input_event_decodes_both_kernel_abis_without_time_t_assumptions() {
    let expected = Event { kind: 3, code: 17, value: i32::MIN };
    for size in [16, 24] {
        let mut bytes = vec![0xa5; size];
        let offset = size - 8;
        bytes[offset..offset + 2].copy_from_slice(&expected.kind.to_ne_bytes());
        bytes[offset + 2..offset + 4].copy_from_slice(&expected.code.to_ne_bytes());
        bytes[offset + 4..].copy_from_slice(&expected.value.to_ne_bytes());

        assert_eq!(Event::decode(&bytes), Some(expected));
    }
    for size in [0, 8, 15, 17, 23, 25] {
        assert_eq!(Event::decode(&vec![0; size]), None);
    }
    assert_eq!(serde_json::to_value(expected).unwrap(), json!({"kind":3,"code":17,"value":i32::MIN}));
}

#[test]
fn retroarch_index_preserves_arrow_button_keyboard_order_and_64_button_limit() {
    assert_eq!(find_retroarch_button_index(&[1, 28, 103, 108, 256, 767], 103), Some(0));
    assert_eq!(find_retroarch_button_index(&[1, 28, 103, 108, 256, 767], 256), Some(2));
    assert_eq!(find_retroarch_button_index(&[1, 28, 103, 108, 256, 767], 28), Some(4));
    assert_eq!(find_retroarch_button_index(&[767], 767), None);
    assert_eq!(find_retroarch_button_index(&(256..=320).collect::<Vec<_>>(), 319), Some(63));
    assert_eq!(find_retroarch_button_index(&(256..=320).collect::<Vec<_>>(), 320), None);
    assert_eq!(find_retroarch_button_index(&[0], 0), None);
    assert_eq!(find_retroarch_button_index(&[], 28), None);
}

#[test]
fn device_metadata_preserves_its_json_shape_and_accepts_limits() {
    let info = create_device_info();

    info.validate().unwrap();

    assert_eq!(serde_json::to_value(&info).unwrap(), json!({
        "path":"/synthetic/event0", "name":"Synthetic pad", "axis_codes":[0,1],
        "key_codes":[28,103,256], "abs_info":{"0":[128,0,255,0,15,0]},
        "identity":{"id/bustype":"0003"},
    }));
    let maximal = DeviceInfo {
        path: "x".repeat(256),
        name: "x".repeat(256),
        axis_codes: (0..64).collect(),
        key_codes: (0..768).collect(),
        abs_info: (0..64).map(|code| (code, [0; 6])).collect(),
        identity: (0..12).map(|index| (format!("{index:064}"), "x".repeat(1024))).collect(),
    };

    maximal.validate().unwrap();
}

#[test]
fn device_metadata_rejects_invalid_capabilities_and_unbounded_details() {
    for axis_codes in [vec![64], vec![0, 0], vec![1, 0], (0..65).collect()] {
        let mut info = create_device_info();
        info.axis_codes = axis_codes;

        assert!(info.validate().is_err());
    }
    for key_codes in [vec![768], vec![28, 28], vec![103, 28], (0..769).collect()] {
        let mut info = create_device_info();
        info.key_codes = key_codes;

        assert!(info.validate().is_err());
    }
    for abs_info in [BTreeMap::from([(2, [0; 6])]), (0..65).map(|code| (code, [0; 6])).collect()] {
        let mut info = create_device_info();
        info.abs_info = abs_info;

        assert!(info.validate().is_err());
    }
    for text in ["x".repeat(257), "Я".repeat(129)] {
        let mut info = create_device_info();
        info.path = text.clone();

        assert!(info.validate().is_err());

        info.path.clear();
        info.name = text;

        assert!(info.validate().is_err());
    }
    for identity in [
        BTreeMap::from([("x".repeat(65), "value".into())]),
        BTreeMap::from([("key".into(), "x".repeat(1025))]),
        (0..13).map(|index| (index.to_string(), "value".into())).collect(),
    ] {
        let mut info = create_device_info();
        info.identity = identity;

        assert_eq!(info.validate().unwrap_err(), "diagnostic device metadata exceeds limits or is invalid");
    }
}
