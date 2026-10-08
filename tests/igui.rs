// SPDX-License-Identifier: BSD-3-Clause
use imanlib::igui::{InputTestAcknowledgement, ScreenshotResult};
use serde_json::json;

#[test]
fn screenshot_result_preserves_the_wire_shape_and_ignores_additional_fields() {
    let result = ScreenshotResult {
        path: "/synthetic/screenshot.png".into(),
        temporary: true,
        width: 1280,
        height: 720,
        format: "png".into(),
    };
    let mut encoded = serde_json::to_value(&result).unwrap();

    assert_eq!(encoded, json!({
        "path":"/synthetic/screenshot.png", "temporary":true,
        "width":1280, "height":720, "format":"png",
    }));

    encoded["extra"] = json!(true);
    let decoded: ScreenshotResult = serde_json::from_value(encoded).unwrap();

    assert_eq!(decoded, result);
}

#[test]
fn input_test_acknowledgement_preserves_the_wire_shape_and_ignores_additional_fields() {
    let acknowledgement = InputTestAcknowledgement {
        handoff_received: true,
        test_started: false,
    };
    let mut encoded = serde_json::to_value(acknowledgement).unwrap();

    assert_eq!(encoded, json!({"handoff_received":true,"test_started":false}));

    encoded["extra"] = json!(true);
    let decoded: InputTestAcknowledgement = serde_json::from_value(encoded).unwrap();

    assert_eq!(decoded, acknowledgement);
}

#[test]
fn gui_results_require_known_fields_and_types() {
    let screenshot = json!({
        "path":"/synthetic/screenshot.png", "temporary":false,
        "width":1280, "height":720, "format":"png", "extra":true,
    });
    for field in ["path", "temporary", "width", "height", "format"] {
        let mut missing = screenshot.clone();
        missing.as_object_mut().unwrap().remove(field);

        assert!(serde_json::from_value::<ScreenshotResult>(missing).is_err(), "{field}");
    }
    for (field, value) in [
        ("path", json!(null)), ("temporary", json!("false")),
        ("width", json!(-1)), ("height", json!(1.5)), ("format", json!(1)),
    ] {
        let mut invalid = screenshot.clone();
        invalid[field] = value;

        assert!(serde_json::from_value::<ScreenshotResult>(invalid).is_err(), "{field}");
    }
    for invalid in [
        json!({"handoff_received":true}), json!({"test_started":false}),
        json!({"handoff_received":1,"test_started":false}),
        json!({"handoff_received":true,"test_started":"false"}),
    ] {
        assert!(serde_json::from_value::<InputTestAcknowledgement>(invalid).is_err());
    }
}
