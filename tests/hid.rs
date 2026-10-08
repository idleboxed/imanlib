// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
mod common;
use {
    imanlib::hid::Control::*,
    imanlib::hid::Decoder,
    imanlib::hid::Profile,
    imanlib::igui::Action,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DESCRIPTOR: &[u8] = include_bytes!("fixtures/shanwan-2563-0555.bin");
fn document() -> Value {
    serde_json::from_slice(&common::control_bytes(common::HID_FILE)).unwrap()
}
fn profile(value: &Value) -> std::io::Result<Profile> {
    Profile::parse(&serde_json::to_vec(value).unwrap())
}
fn decode() -> Decoder {
    Decoder::new(&common::hid_profile(), DESCRIPTOR).unwrap()
}
fn packet(id: u8, buttons: u16, hat: u8) -> [u8; 8] {
    [
        id,
        128,
        128,
        128,
        128,
        hat | ((buttons as u8 & 15) << 4),
        (buttons >> 4) as u8,
        0,
    ]
}

#[test]
fn measured_descriptor_and_capture_integrity() {
    assert_eq!(
        format!("{:x}", Sha256::digest(DESCRIPTOR)),
        "cabd2b0847144b2596be53a7fdc5d281512357ca1bb3b44ee29816b126b5cf26"
    );
    assert_eq!(DESCRIPTOR.len(), 202);
    assert_eq!(decode().report_ids().collect::<Vec<_>>(), [1, 2]);
}
#[test]
fn all_buttons_on_both_channels_follow_profile() {
    let decoder = decode();
    for id in [1, 2] {
        for (bit, control) in [
            North,
            East,
            South,
            West,
            LeftShoulder,
            RightShoulder,
            LeftTrigger,
            RightTrigger,
            Back,
            Start,
            LeftStick,
            RightStick,
        ]
        .into_iter()
        .enumerate()
        {
            let (channel, state) = decoder.decode(&packet(id, 1 << bit, 15)).unwrap();
            assert_eq!(channel, id);
            assert_eq!(state.value(control), 1);
            assert!(!state.neutral());
        }
        assert!(decoder.decode(&packet(id, 0, 15)).unwrap().1.neutral());
    }
}

#[test]
fn start_on_both_hid_channels_is_a_launch_not_confirmation_or_trash() {
    let decoder = decode();
    for id in [1, 2] {
        let state = decoder.decode(&packet(id, 1 << 9, 15)).unwrap().1;
        let mut expected = imanlib::input::Buttons::default();
        expected.set(Action::Start, true);
        assert_eq!(state.menu_buttons(), expected);
    }
}
#[test]
fn hats_including_all_null_values_and_axes() {
    let decoder = decode();
    for hat in 0..16 {
        let state = decoder.decode(&packet(1, 0, hat)).unwrap().1;
        assert_eq!(state.value(Dpad), if hat < 8 { hat as i32 } else { -1 });
        assert_eq!(state.neutral(), hat >= 8);
    }
    let mut p = packet(2, 0, 15);
    p[1] = 0;
    p[2] = 255;
    p[3] = 255;
    p[4] = 0;
    let state = decoder.decode(&p).unwrap().1;
    assert_eq!(state.value(RightStickX), -32767);
    assert_eq!(state.value(RightStickY), 32767);
    assert_eq!(state.value(LeftStickX), 32767);
    assert_eq!(state.value(LeftStickY), -32767);
    assert!(state.menu_buttons().down(Action::Up));
    assert!(state.menu_buttons().down(Action::Right));
    p[5] = 4;
    assert!(decoder
        .decode(&p)
        .unwrap()
        .1
        .menu_buttons()
        .down(Action::Down));
}
#[test]
fn profile_remaps_without_rust_changes() {
    let mut p = document();
    p["controls"]["west"] = json!("Button.1");
    p["controls"]["north"] = json!("Button.4");
    let decoder = Decoder::new(&profile(&p).unwrap(), DESCRIPTOR).unwrap();
    assert!(decoder
        .decode(&packet(1, 1, 15))
        .unwrap()
        .1
        .menu_buttons()
        .down(Action::LowerFilters));
}
#[test]
fn changed_descriptor_moves_fields_without_fingerprint_or_fixed_offsets() {
    let mut descriptor = Vec::new();
    for half in DESCRIPTOR.chunks(101) {
        descriptor.extend_from_slice(&half[..10]);
        descriptor.extend_from_slice(&[0x75, 8, 0x95, 1, 0x81, 3]); // synthetic constant byte
        descriptor.extend_from_slice(&half[10..]);
    }
    let decoder = Decoder::new(&common::hid_profile(), &descriptor).unwrap();
    for id in [1, 2] {
        let mut moved = vec![id, 0xa5];
        moved.extend_from_slice(&packet(id, 8, 15)[1..]);
        assert!(decoder
            .decode(&moved)
            .unwrap()
            .1
            .menu_buttons()
            .down(Action::LowerFilters));
    }
}
#[test]
fn unnumbered_signed_axes_and_short_packets() {
    let mut p = document();
    p["controllers"] = json!([{"input_report_id":0}]);
    p["controls"] = json!({"north":"Button.1","east":"Button.2","south":"Button.3","west":"Button.4","left_stick_x":"GenericDesktop.X","left_stick_y":"GenericDesktop.Y"});
    let descriptor = [
        5, 1, 9, 5, 0xa1, 1, 0x15, 0x81, 0x25, 0x7f, 0x75, 8, 0x95, 2, 9, 0x30, 9, 0x31, 0x81, 2,
        5, 9, 0x15, 0, 0x25, 1, 0x19, 1, 0x29, 4, 0x75, 1, 0x95, 4, 0x81, 2, 0x75, 4, 0x95, 1,
        0x81, 3, 0xc0,
    ];
    let decoder = Decoder::new(&profile(&p).unwrap(), &descriptor).unwrap();
    assert!(decoder.decode(&[0, 0, 0]).unwrap().1.neutral());
    let (id, state) = decoder.decode(&[0x81, 0x7f, 8]).unwrap();
    assert_eq!(id, 0);
    assert_eq!(state.value(LeftStickX), -32767);
    assert_eq!(state.value(LeftStickY), 32767);
    assert!(decoder.decode(&[0x80, 0, 0]).is_err()); // outside advertised -127..127
    assert!(decoder.decode(&[0, 0]).is_err());
}
#[test]
fn malformed_profiles_fail_closed() {
    for (key, value) in [
        ("backend", json!("usb-write")),
        ("access", json!("read-write")),
        ("name", json!("")),
        ("controllers", json!([])),
        (
            "controllers",
            json!([{"input_report_id":1},{"input_report_id":1}]),
        ),
        (
            "controllers",
            json!([{"input_report_id":0},{"input_report_id":1}]),
        ),
    ] {
        let mut p = document();
        p[key] = value;
        assert!(profile(&p).is_err(), "{p}");
    }
    for usage in [
        "Button.0",
        "Button.65536",
        "GenericDesktop.X",
        "Button.1",
        "Keyboard.X",
    ] {
        let mut p = document();
        p["controls"]["west"] = json!(usage);
        assert!(profile(&p).is_err());
    }
    for key in ["west", "left_stick_x"] {
        let mut p = document();
        p["controls"].as_object_mut().unwrap().remove(key);
        assert!(profile(&p).is_err());
    }
    let duplicate = String::from_utf8(common::control_bytes(common::HID_FILE))
        .unwrap()
        .replace(
            "\"west\": \"Button.4\"",
            "\"west\": \"Button.4\", \"west\": \"Button.4\"",
        );
    assert!(Profile::parse(duplicate.as_bytes()).is_err());
    assert!(Profile::parse(&vec![b' '; 32769]).is_err());
}

#[test]
fn profile_ignores_unknown_fields_and_requires_known_fields() {
    let mut p = document();
    p["version"] = json!(999);
    p["future"] = json!(true);
    p["match"]["future"] = json!(true);
    p["controllers"][0]["future"] = json!(true);
    assert!(profile(&p).is_ok());

    p.as_object_mut().unwrap().remove("backend");
    assert!(profile(&p)
        .err()
        .unwrap()
        .to_string()
        .contains("missing field `backend`"));
}
#[test]
fn descriptor_missing_ambiguous_relative_and_resource_limits_are_rejected() {
    let mut missing = DESCRIPTOR.to_vec();
    missing[29] = 0x33;
    let mut ambiguous = DESCRIPTOR.to_vec();
    ambiguous[27] = 0x30;
    let mut relative = DESCRIPTOR.to_vec();
    relative[33] = 6;
    for bytes in [
        missing,
        ambiguous,
        relative,
        vec![],
        vec![0; 4097],
        vec![0xfe, 0, 0],
        vec![0x97, 255, 255, 255, 255],
        vec![0x75, 33],
        vec![0xa4],
        vec![0xb4],
        vec![0x27],
        [0xa1, 1].repeat(5),
        vec![0xc0],
    ] {
        assert!(
            Decoder::new(&common::hid_profile(), &bytes).is_err(),
            "{bytes:?}"
        );
    }
}
#[test]
fn wrong_report_id_size_or_channel_definition_is_rejected() {
    let decoder = decode();
    for p in [vec![], vec![3; 8], vec![1; 7], vec![1; 9]] {
        assert!(decoder.decode(&p).is_err());
    }
    let mut p = document();
    p["controllers"] = json!([{"input_report_id":1}]);
    assert!(Decoder::new(&profile(&p).unwrap(), DESCRIPTOR).is_err());
}
#[test]
fn replay_measured_simultaneous_two_pads_without_state_merging() {
    let decoder = decode();
    let mut states = [imanlib::hid::State::default(); 2];
    let mut simultaneous = 0;
    for packet in include_bytes!("fixtures/shanwan-two-pads.bin").chunks_exact(8) {
        let (id, state) = decoder.decode(packet).unwrap();
        states[usize::from(id - 1)] = state;
        if states[0].value(West) == 1 && states[1].value(East) == 1 {
            simultaneous += 1;
        }
        assert_eq!(states[0].value(East), 0);
        assert_eq!(states[1].value(West), 0);
    }
    assert_eq!(simultaneous, 247);
    assert!(states.iter().all(|s| s.neutral()));
}
#[test]
fn profile_loading_rejects_nonfiles_symlinks_and_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.json");
    std::fs::write(&path, &common::control_bytes(common::HID_FILE)).unwrap();
    assert!(Profile::load(&path)
        .unwrap()
        .matches(3, 0x2563, 0x0555, "ShanWan Twin USB Joystick"));
    assert!(Profile::load(dir.path()).is_err());
    assert!(Profile::load(&dir.path().join("absent")).is_err());
    #[cfg(target_os = "linux")]
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Profile::load(&link).is_err());
    }
    std::fs::write(&path, vec![b' '; 32769]).unwrap();
    assert!(Profile::load(&path).is_err());
}
