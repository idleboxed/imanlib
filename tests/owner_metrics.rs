// SPDX-License-Identifier: BSD-3-Clause
use imanlib::{
    display_metrics::{EventStamp, Summary, Timings},
    igui::{Bootstrap, InputRequest, ScreenshotRequest},
    input_metrics::{Counters, LogicalEdgeSampler, LogicalEdges, Sample, CONTROL_LIMIT, LOGICAL_EDGE_COUNT},
};
use serde_json::json;

#[test]
fn display_events_keep_raw_clock_wrap_regression_and_missing_samples_distinct() {
    let mut s = Summary::default();
    for (sequence, timestamp_us) in [(u32::MAX, 100), (0, 110), (2, 90)] {
        s.record(Timings {
            copy_us: Some(0),
            event: Some(EventStamp {
                sequence,
                timestamp_us,
            }),
            ..Default::default()
        });
    }
    assert_eq!(s.steps, 3);
    assert_eq!(s.metrics[0].count, 0);
    assert_eq!(s.metrics[2].count, 3);
    assert_eq!(s.sequence_delta.sum, 3);
    assert_eq!(s.timestamp_delta_us.sum, 10);
    assert_eq!(s.timestamp_regressions, 1);
    let encoded = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<Summary>(&encoded).unwrap().steps, 3);
}

#[test]
fn passive_input_ranges_are_bounded_and_payloads_ignore_unknown_fields() {
    let mut c = Counters::default();
    for i in 0..20 {
        c.event(3, i, i32::from(i));
    }
    c.event(3, 0, -7);
    assert_eq!(c.controls.len(), CONTROL_LIMIT);
    assert_eq!(c.controls_omitted, 12);
    assert_eq!(c.controls[0].min, -7);
    let mut value = serde_json::to_value(Sample::default()).unwrap();
    value["pid"] = json!(99);
    let sample: Sample = serde_json::from_value(value).unwrap();
    assert!(sample.validate());
    assert!(!serde_json::to_value(sample).unwrap().as_object().unwrap().contains_key("pid"));
}

#[test]
fn input_edges_roundtrip_all_current_counters_without_changing_the_wire_shape() {
    let counts: LogicalEdges = std::array::from_fn(|index| index as u64 + 1);
    let mut value = serde_json::to_value(Sample::default()).unwrap();
    value["logical_edges"] = json!(counts);

    let sample: Sample = serde_json::from_value(value.clone()).unwrap();

    assert!(sample.validate());
    assert_eq!(sample.logical_edges, counts);
    assert_eq!(sample.logical_edges[9], 10);
    assert_eq!(serde_json::to_value(sample).unwrap(), value);
}

#[test]
fn input_edges_reject_other_lengths_and_non_counter_values() {
    for length in [0, 1, 8, 9, 11, 32, 1024] {
        let mut value = serde_json::to_value(Sample::default()).unwrap();
        value["logical_edges"] = json!(vec![0; length]);

        assert!(serde_json::from_value::<Sample>(value).is_err(), "length {length}");
    }
    for invalid in [json!(-1), json!(1.5), json!("1"), json!(null), json!(true)] {
        let mut value = serde_json::to_value(Sample::default()).unwrap();
        value["logical_edges"][9] = invalid;

        assert!(serde_json::from_value::<Sample>(value).is_err());
    }
}

#[test]
fn input_sampler_counts_all_sampled_edges_once_and_counts_start_again_after_release() {
    let mut sampler = LogicalEdgeSampler::default();

    sampler.record([false; LOGICAL_EDGE_COUNT]);
    sampler.record([true; LOGICAL_EDGE_COUNT]);
    sampler.record([true; LOGICAL_EDGE_COUNT]);

    assert_eq!(sampler.snapshot(), [1; LOGICAL_EDGE_COUNT]);

    sampler.record([false; LOGICAL_EDGE_COUNT]);
    let mut start = [false; LOGICAL_EDGE_COUNT];
    start[9] = true;
    sampler.record(start);
    sampler.record(start);

    let counts = sampler.snapshot();
    assert_eq!(&counts[..9], [1; 9]);
    assert_eq!(counts[9], 2);
}

#[test]
fn bootstrap_requires_all_current_fields() {
    let mut value = json!({
        "diag_display": false,
        "diag_input": false,
        "preferences_writable": false,
        "root": "/tmp",
        "state": {},
        "notice": null
    });
    value["state"] = serde_json::to_value(imanlib::igui::GuiState::default()).unwrap();
    value["extra"] = json!(true);
    value["state"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Bootstrap>(value.clone()).is_ok());
    value.as_object_mut().unwrap().remove("diag_input");
    assert!(serde_json::from_value::<Bootstrap>(value).is_err());
}

#[test]
fn gui_command_parameters_ignore_unknown_fields_but_keep_required_fields_and_types() {
    let input: InputRequest = serde_json::from_value(json!({"action":"down","extra":true})).unwrap();
    assert_eq!(input.action, "down");
    assert!(serde_json::from_value::<InputRequest>(json!({"extra":true})).is_err());
    assert!(serde_json::from_value::<InputRequest>(json!({"action":1,"extra":true})).is_err());

    let screenshot: ScreenshotRequest = serde_json::from_value(json!({"path":"/tmp/example.png","extra":true})).unwrap();
    assert_eq!(screenshot.path.unwrap(), std::path::Path::new("/tmp/example.png"));
    assert!(serde_json::from_value::<ScreenshotRequest>(json!({"path":1,"extra":true})).is_err());
}

#[test]
fn input_test_capability_is_opt_in_and_rejects_invalid_values() {
    let mut value = json!({
        "diag_display": false,
        "diag_input": false,
        "preferences_writable": false,
        "root": "/tmp",
        "state": imanlib::igui::GuiState::default(),
        "notice": null
    });
    assert!(!serde_json::from_value::<Bootstrap>(value.clone()).unwrap().input_test);

    value["input_test"] = json!(true);
    let bootstrap = serde_json::from_value::<Bootstrap>(value.clone()).unwrap();
    assert!(bootstrap.input_test);
    assert!(!bootstrap.diag_input);

    value["input_test"] = json!("true");
    assert!(serde_json::from_value::<Bootstrap>(value).is_err());
}
