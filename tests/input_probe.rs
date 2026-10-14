// SPDX-License-Identifier: BSD-3-Clause
#![cfg(feature = "input")]
//! Synthetic evdev packets. These tests do not establish a physical mapping.
use {
    imanlib::input_probe::*,
    imanlib::surface::Surface,
};
use imanlib::evdev::{DeviceInfo, Event};
use serde_json::json;
use std::collections::BTreeMap;
fn info(index: usize) -> DeviceInfo {
    DeviceInfo {
        path: format!("/synthetic/event{index}"),
        name: "Synthetic twin receiver".into(),
        axis_codes: vec![0, 1],
        key_codes: vec![
            19, 21, 28, 30, 38, 44, 45, 46, 48, 54, 103, 105, 106, 107, 108,
        ],
        abs_info: BTreeMap::from([(0, [128, 0, 255, 0, 15, 0]), (1, [128, 0, 255, 0, 15, 0])]),
        identity: BTreeMap::new(),
    }
}
fn event(kind: u16, code: u16, value: i32) -> Event {
    Event { kind, code, value }
}
fn four() -> Probe {
    let mut probe_item = Probe::default();
    for index_item in 0..4 {
        probe_item.connect(info(index_item), 0).unwrap();
        probe_item.baseline(index_item, &[], 0).unwrap();
    }
    probe_item
}
fn tap(probe_item: &mut Probe, slot: usize, code: u16, ms: u64) {
    probe_item.event(slot, event(1, code, 1), ms);
    probe_item.event(slot, event(1, code, 0), ms + 1);
    probe_item.sampled(ms + 1);
}
fn bound() -> Probe {
    let mut probe_item = four();
    tap(&mut probe_item, 0, 28, 10);
    tap(&mut probe_item, 0, 28, 20);
    probe_item
}
fn selected() -> Probe {
    let mut probe_item = bound();
    tap(&mut probe_item, 0, 30, 30);
    tap(&mut probe_item, 0, 28, 40);
    probe_item
}
fn together() -> Probe {
    let mut probe_item = bound();
    for number in 0..4 {
        tap(&mut probe_item, 0, 28, 50 + number * 10);
    }
    assert_eq!(probe_item.stage, Stage::Together);
    probe_item
}

#[test]
fn steps_never_advance_with_time_and_start_is_learned_not_hardcoded() {
    let mut probe_item = four();
    probe_item.sampled(500_000);
    assert_eq!(probe_item.stage, Stage::Prepare);
    tap(&mut probe_item, 1, 54, 500_010);
    assert_eq!(probe_item.stage, Stage::Prepare);
    tap(&mut probe_item, 1, 54, 500_020);
    assert_eq!(probe_item.stage, Stage::Player { player: 1 });
    assert_eq!(
        probe_item.guide.operator,
        Some(Key {
            generation: 2,
            code: 54
        })
    );
    probe_item.sampled(END_MS);
    assert_eq!(probe_item.stage, Stage::Player { player: 1 });
}
#[test]
fn held_initial_state_repeats_and_unpaired_releases_do_not_bind_start() {
    let mut probe_item = four();
    probe_item.baseline(0, &[28], 0).unwrap();
    for value_item in [1, 2, 2, 0, 0] {
        probe_item.event(0, event(1, 28, value_item), 10);
        probe_item.sampled(10);
    }
    assert!(probe_item.guide.operator.is_none());
    tap(&mut probe_item, 0, 28, 20);
    assert!(probe_item.guide.operator.is_none());
    tap(&mut probe_item, 0, 28, 30);
    assert!(probe_item.guide.operator.is_some());
}
#[test]
fn start_binding_rejects_multiple_taps_from_one_backlogged_sample() {
    let mut probe_item = four();
    for _ in 0..2 {
        probe_item.event(0, event(1, 28, 1), 1);
        probe_item.event(0, event(1, 28, 0), 2);
    }
    probe_item.sampled(2);
    assert!(probe_item.guide.operator.is_none());
    tap(&mut probe_item, 0, 28, 10);
    assert!(probe_item.guide.operator.is_none());
}
#[test]
fn all_four_players_have_nine_explicitly_confirmed_steps_with_raw_key_codes() {
    let mut probe_item = bound();
    let mut ms = 100;
    let codes = [103, 108, 105, 106, 21, 48, 45, 30, 54];
    let mut budget = LogBudget::default();
    for player in 1..=4 {
        let slot = player - 1;
        tap(&mut probe_item, slot, 30, ms);
        ms += 10;
        tap(&mut probe_item, 0, 28, ms);
        ms += 10;
        for (index_item, &code) in codes.iter().enumerate() {
            assert_eq!(
                probe_item.stage,
                Stage::Button {
                    player: player as u8,
                    action: ACTIONS[index_item]
                }
            );
            tap(&mut probe_item, slot, code, ms);
            ms += 10;
            assert!(matches!(probe_item.stage, Stage::Button { .. }));
            tap(&mut probe_item, 0, 28, ms);
            ms += 10;
            for report in probe_item.take_records() {
                assert!(budget.encode(&report).is_some());
            }
        }
    }
    assert_eq!(probe_item.stage, Stage::Together);
    assert_eq!(probe_item.guide.players, [Some(1), Some(2), Some(3), Some(4)]);
    for slot in 0..4 {
        probe_item.event(slot, event(1, 21, 1), ms);
    }
    probe_item.sampled(ms);
    assert_eq!(probe_item.max_overlap, 4);
    for slot in 0..4 {
        probe_item.event(slot, event(1, 21, 0), ms + 1);
    }
    probe_item.sampled(ms + 1);
    tap(&mut probe_item, 0, 28, ms + 10);
    assert_eq!(probe_item.stage, Stage::Finish);
    assert!(!probe_item.done(ms + 20));
    assert!(probe_item.done(ms + 2011));
    let result = probe_item.finish(ms + 2020, "synthetic");
    for report in probe_item.take_records() {
        assert!(budget.encode(&report).is_some());
    }
    assert_eq!(result["physical_players_verified"], false);
    assert_eq!(result["mapping_written"], false);
    assert_eq!(
        result["operator_confirmed_steps"].as_array().unwrap().len(),
        36
    );
    assert!(result.to_string().len() < 8192);
    assert_eq!(budget.omitted, 0);
}
#[test]
fn start_without_candidate_skips_step_or_whole_missing_player() {
    let mut probe_item = selected();
    tap(&mut probe_item, 0, 28, 50);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Down"
        }
    );
    assert!(probe_item
        .take_records()
        .iter()
        .any(|report| report["event"] == "step_result" && report["skipped"] == true));
    let probe_item = together();
    assert_eq!(probe_item.guide.players, [None; 4]);
}
#[test]
fn multiple_keys_wrong_player_and_reused_player_are_not_accepted_as_mapping() {
    let mut probe_item = selected();
    tap(&mut probe_item, 0, 103, 50);
    tap(&mut probe_item, 0, 108, 60);
    assert!(probe_item.guide.ambiguous);
    tap(&mut probe_item, 0, 28, 70);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Up"
        }
    );
    assert!(probe_item.guide.candidate.is_none());
    tap(&mut probe_item, 1, 103, 80);
    tap(&mut probe_item, 0, 28, 90);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Up"
        }
    );
    assert!(probe_item.guide.candidate.is_none());
    for index_item in 0..9 {
        tap(&mut probe_item, 0, 28, 100 + index_item * 10);
    }
    tap(&mut probe_item, 0, 30, 200);
    tap(&mut probe_item, 0, 28, 210);
    assert_eq!(probe_item.stage, Stage::Player { player: 2 });
    assert_eq!(probe_item.guide.players[1], None);
}
#[test]
fn candidate_and_confirmation_in_same_sample_cannot_advance_a_step() {
    let mut probe_item = selected();
    for (slot, code) in [(0, 28), (0, 103)] {
        probe_item.event(slot, event(1, code, 1), 50);
        probe_item.event(slot, event(1, code, 0), 51);
    }
    probe_item.sampled(52);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Up"
        }
    );
    tap(&mut probe_item, 0, 28, 60);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Down"
        }
    );
}
#[test]
fn held_start_finishes_but_preexisting_held_keys_do_not_leak_between_steps() {
    let mut probe_item = bound();
    probe_item.event(0, event(1, 28, 1), 100);
    probe_item.sampled(2099);
    assert_ne!(probe_item.stage, Stage::Finish);
    probe_item.sampled(2100);
    assert_eq!(probe_item.stage, Stage::Finish);
    let mut probe_item = together();
    probe_item.baseline(0, &[21], 100).unwrap();
    probe_item.sampled(101);
    assert_eq!(probe_item.max_overlap, 0);
    probe_item.event(0, event(1, 21, 0), 102);
    probe_item.sampled(102);
    probe_item.event(0, event(1, 21, 1), 103);
    probe_item.sampled(103);
    assert_eq!(probe_item.max_overlap, 1);
}
#[test]
fn short_taps_are_evidence_but_not_simultaneous_holds_and_axes_are_not_keys() {
    let mut probe_item = together();
    tap(&mut probe_item, 0, 21, 100);
    tap(&mut probe_item, 1, 21, 110);
    probe_item.event(2, event(3, 1, i32::MAX), 120);
    probe_item.sampled(120);
    assert_eq!(probe_item.max_overlap, 0);
    assert!(probe_item.guide.candidate.is_none());
    assert_eq!(
        probe_item.streams[2].as_ref().unwrap().last,
        Some(event(3, 1, i32::MAX))
    );
}
#[test]
fn resync_and_incomplete_poll_discard_candidates_and_require_fresh_neutral_state() {
    let mut probe_item = selected();
    tap(&mut probe_item, 0, 103, 50);
    probe_item.resync(0, 60, "synthetic SYN_DROPPED");
    assert!(probe_item.guide.candidate.is_none());
    assert!(!probe_item.ready(0));
    tap(&mut probe_item, 0, 28, 70);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Up"
        }
    );
    probe_item.baseline(0, &[28], 80).unwrap();
    probe_item.event(0, event(1, 28, 0), 90);
    probe_item.sampled(90);
    assert_eq!(
        probe_item.stage,
        Stage::Button {
            player: 1,
            action: "Up"
        }
    );
    probe_item.incomplete(0, 100);
    assert_eq!(probe_item.incomplete_polls, 1);
    assert!(!probe_item.ready(0));
}
#[test]
fn disconnect_after_finish_keeps_confirmed_observations_for_export() {
    let mut probe_item = selected();
    tap(&mut probe_item, 0, 103, 50);
    tap(&mut probe_item, 0, 28, 60);
    probe_item.event(0, event(1, 28, 1), 100);
    probe_item.sampled(2100);
    assert_eq!(probe_item.stage, Stage::Finish);
    assert_eq!(probe_item.guide.observations.len(), 1);

    probe_item.disconnect(0, 2200, "synthetic unplug during finish message");

    assert_eq!(probe_item.stage, Stage::Finish);
    assert_eq!(probe_item.guide.observations.len(), 1);
    assert_eq!(probe_item.guide.observations[0].action, "Up");
    assert!(probe_item.done(4100));
}
#[test]
fn disconnecting_operator_resets_guide_and_reused_path_gets_a_new_generation() {
    let mut probe_item = selected();
    probe_item.disconnect(0, 50, "synthetic unplug");
    assert_eq!(probe_item.stage, Stage::Prepare);
    assert!(probe_item.guide.operator.is_none());
    assert_eq!(probe_item.connect(info(0), 60).unwrap(), 0);
    assert_eq!(probe_item.streams[0].as_ref().unwrap().generation, 5);
}
#[test]
fn disconnecting_selected_nonoperator_player_requires_selection_again() {
    let mut probe_item = bound();
    tap(&mut probe_item, 1, 30, 30);
    tap(&mut probe_item, 0, 28, 40);
    probe_item.disconnect(1, 50, "synthetic unplug");
    assert_eq!(probe_item.stage, Stage::Player { player: 1 });
    assert_eq!(probe_item.guide.players[0], None);
}
#[test]
fn metadata_invalid_events_connection_counts_and_noise_logs_are_bounded() {
    let mut probe_item = Probe::default();
    let mut bad = info(0);
    bad.key_codes = vec![768];
    assert!(probe_item.connect(bad, 0).is_err());
    let mut bad = info(0);
    bad.axis_codes = vec![0, 0];
    assert!(probe_item.connect(bad, 0).is_err());
    probe_item.connect(info(0), 0).unwrap();
    assert!(probe_item.connect(info(0), 0).is_err());
    probe_item.baseline(0, &[], 0).unwrap();
    probe_item.event(0, event(1, 0xffff, 1), 1);
    assert_eq!(probe_item.streams[0].as_ref().unwrap().evidence.invalid_events, 1);
    for index_item in 1..MAX_STREAMS {
        probe_item.connect(info(index_item), 0).unwrap();
    }
    assert!(probe_item.connect(info(9), 0).is_err());
    while probe_item.connections < MAX_CONNECTIONS {
        probe_item.disconnect(0, 0, "synthetic");
        probe_item.connect(info(0), 0).unwrap();
    }
    probe_item.disconnect(0, 0, "synthetic");
    assert!(probe_item.connect(info(0), 0).is_err());
    let mut budget = LogBudget::default();
    for _ in 0..10000 {
        budget.encode(&json!({"noise":"x".repeat(1000)}));
    }
    assert!(budget.bytes <= LOG_BYTES);
    assert!(budget.omitted > 0);
}
#[test]
fn raw_controls_preserve_full_key_codes_and_limit_distinct_evidence() {
    let mut probe_item = Probe::default();
    let mut metadata = info(0);
    metadata.key_codes = (0..768).collect();
    probe_item.connect(metadata, 0).unwrap();
    probe_item.baseline(0, &[], 0).unwrap();
    for code in 0..768 {
        probe_item.event(0, event(1, code, 1), 1);
    }
    let stream = probe_item.streams[0].as_ref().unwrap();
    assert_eq!(stream.evidence.controls.len(), 32);
    assert_eq!(stream.evidence.controls_omitted, 736);
    assert!(probe_item.baseline(0, &[768], 2).is_err());
}
#[test]
fn render_distinguishes_manual_prompts_candidates_and_finish() {
    let mut probe_item = four();
    let mut frame = Surface::new(1280, 720);
    draw(&probe_item, 0, &mut frame).unwrap();
    let first = frame.pixels().to_vec();
    tap(&mut probe_item, 0, 28, 10);
    tap(&mut probe_item, 0, 28, 20);
    draw(&probe_item, 30, &mut frame).unwrap();
    assert_ne!(first, frame.pixels());
    let second = frame.pixels().to_vec();
    tap(&mut probe_item, 0, 30, 40);
    draw(&probe_item, 50, &mut frame).unwrap();
    assert_ne!(second, frame.pixels());
}

#[test]
fn compact_report_preserves_invalid_input_loss_and_disconnect_without_claiming_players() {
    let mut probe = four();
    probe.event(0, event(3, 0, 255), 1);
    probe.event(0, event(1, 768, 1), 2);
    probe.resync(1, 3, "SYN_DROPPED");
    probe.disconnect(0, 4, "synthetic unplug");

    let sample = probe.diagnostic_report(false);

    assert!(sample.validate());
    assert_eq!(sample.evdev.connections, 4);
    assert_eq!(sample.evdev.disconnects, 1);
    assert_eq!(sample.evdev.events, 2);
    assert_eq!(sample.evdev.errors, 1);
    assert_eq!(sample.evdev.syn_dropped, 1);
    assert_eq!(sample.probe.as_ref().unwrap().max_connected, 4);
    assert!(sample.evdev_connected);
    assert!(sample.channels.is_empty());
    assert_eq!(sample.logical_edges, imanlib::input_metrics::LogicalEdges::default());
}
