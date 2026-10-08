// SPDX-License-Identifier: BSD-3-Clause
#![allow(dead_code)]

use imanlib::{gamepad::profile::Profile as EvdevProfile, hid::Profile as HidProfile};
use serde_json::json;

pub const HID_FILE: &str = "sample-0-hid.json";
pub const BOARD_EVDEV_FILE: &str = "sample-1-digital-evdev.json";
pub const PC_EVDEV_FILE: &str = "sample-2-button-evdev.json";

/// Test-only profiles matching the recorded identities and synthetic events.
/// These are not the profiles distributed with either application.
pub fn control_bytes(name: &str) -> Vec<u8> {
    let mut document = json!({
        "name": "ShanWan Twin USB Joystick",
        "backend": "evdev",
        "access": "read-only",
        "match": {"bus": "usb", "vendor": "2563", "product": "0555"}
    });

    match name {
        HID_FILE => {
            document["backend"] = "hidraw".into();
            document["controllers"] = json!([
                {"input_report_id": 1}, {"input_report_id": 2}
            ]);
            document["controls"] = json!({
                "south": "Button.3", "east": "Button.2", "west": "Button.4", "north": "Button.1",
                "back": "Button.9", "start": "Button.10",
                "left_stick_x": "GenericDesktop.X", "left_stick_y": "GenericDesktop.Y",
                "right_stick_x": "GenericDesktop.Z", "right_stick_y": "GenericDesktop.Rz",
                "dpad": "GenericDesktop.HatSwitch",
                "left_shoulder": "Button.5", "right_shoulder": "Button.6",
                "left_trigger": "Button.7", "right_trigger": "Button.8",
                "left_stick": "Button.11", "right_stick": "Button.12"
            });
        }
        BOARD_EVDEV_FILE => {
            document["match"]["vendor"] = "20bc".into();
            document["match"]["product"] = "5500".into();
            document["buttons"] = json!({
                "west": 45, "east": 48, "north": 21, "south": 30, "back": 54, "start": 28,
                "up": 103, "down": 108, "left": 105, "right": 106
            });
        }
        PC_EVDEV_FILE => {
            document["buttons"] = json!({
                "west": 291, "east": 289, "north": 288, "south": 290, "back": 296, "start": 297
            });
        }
        _ => panic!("unknown input fixture: {name}"),
    }

    if name != HID_FILE {
        document["axes"] = json!({
            "left_stick_x": 0, "left_stick_y": 1, "dpad_x": 16, "dpad_y": 17
        });
    }

    serde_json::to_vec_pretty(&document).unwrap()
}

pub fn hid_profile() -> HidProfile {
    HidProfile::parse(&control_bytes(HID_FILE)).unwrap()
}

pub fn evdev_profiles() -> Vec<EvdevProfile> {
    [BOARD_EVDEV_FILE, PC_EVDEV_FILE].into_iter()
        .map(|name| EvdevProfile::parse(&control_bytes(name)).unwrap())
        .collect()
}
