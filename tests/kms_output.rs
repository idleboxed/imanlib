// SPDX-License-Identifier: BSD-3-Clause
#![cfg(all(feature = "drm", target_os = "linux"))]

use {
    imanlib::kms::output::Connector,
    imanlib::kms::output::Crtc,
    imanlib::kms::output::DisplayMode,
    imanlib::kms::output::Encoder,
    imanlib::kms::output::Request,
    imanlib::kms::output::Timing,
    imanlib::kms::output::Topology,
};

// Synthetic topology, not a recording of the board's unlogged encoder/CRTC state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mode {
    timing: Timing,
    preferred: bool,
    metadata: u8,
}

impl DisplayMode for Mode {
    fn timing(self) -> Timing {
        self.timing
    }
    fn preferred(self) -> bool {
        self.preferred
    }
}

fn mode() -> Mode {
    Mode {
        timing: Timing {
            size: (1280, 720),
            clock: 74250,
            horizontal: (1390, 1430, 1650),
            vertical: (725, 730, 750),
            hskew: 0,
            vscan: 0,
            flags: 5,
        },
        preferred: false,
        metadata: 0,
    }
}

fn disabled() -> Topology<Mode> {
    Topology {
        connectors: vec![Connector {
            id: 62,
            connected: true,
            hdmi: true,
            current_encoder: None,
            encoders: vec![61],
            modes: vec![mode()],
        }],
        encoders: vec![Encoder {
            id: 61,
            current_crtc: None,
            possible_crtcs: vec![59],
        }],
        crtcs: vec![Crtc {
            id: 59,
            framebuffer: None,
            position: (0, 0),
            mode: None,
        }],
    }
}

fn active() -> Topology<Mode> {
    let mut topology = disabled();
    topology.connectors[0].current_encoder = Some(61);
    topology.encoders[0].current_crtc = Some(59);
    topology.crtcs[0].framebuffer = Some(80);
    topology.crtcs[0].position = (11, 22);
    topology.crtcs[0].mode = Some(mode());
    topology
}

#[test]
fn missing_current_encoder_selects_advertised_unused_route() {
    let topology = disabled();

    let output = topology.select().unwrap();

    assert_eq!(
        (output.connector, output.encoder, output.previous.id),
        (62, 61, 59)
    );
    assert_eq!(output.previous.mode, None);
    assert_eq!(output.mode, topology.connectors[0].modes[0]);
}

#[test]
fn current_active_route_and_exact_timing_win_over_advertised_preferred_mode() {
    let mut topology = active();
    topology.connectors[0].modes[0].preferred = true;
    topology.connectors[0].modes[0].timing.clock = 74176;

    let output = topology.select().unwrap();

    assert_eq!(output.mode, mode());
    assert_eq!(output.previous, topology.crtcs[0]);
}

#[test]
fn current_disabled_route_is_retained_even_when_advertised_later() {
    let mut topology = disabled();
    topology.connectors[0].current_encoder = Some(61);
    topology.connectors[0].encoders.insert(0, 71);
    topology.encoders[0].current_crtc = Some(59);
    topology.encoders.push(Encoder {
        id: 71,
        current_crtc: None,
        possible_crtcs: vec![69],
    });
    topology.crtcs.insert(
        0,
        Crtc {
            id: 69,
            framebuffer: None,
            mode: None,
            position: (0, 0),
        },
    );

    let output = topology.select().unwrap();

    assert_eq!((output.encoder, output.previous.id), (61, 59));
}

#[test]
fn missing_current_crtc_uses_compatible_disabled_crtc() {
    let mut topology = disabled();
    topology.connectors[0].current_encoder = Some(61);

    let output = topology.select().unwrap();

    assert_eq!(output.previous.id, 59);
}

#[test]
fn preferred_720p_is_selected_without_guessing_a_timing() {
    let mut topology = disabled();
    let mut preferred = mode();
    preferred.preferred = true;
    preferred.timing.clock = 74176;
    let mut hd1080 = preferred;
    hd1080.timing.size = (1920, 1080);
    topology.connectors[0].modes = vec![hd1080, mode(), preferred];

    let output = topology.select().unwrap();

    assert_eq!(output.mode, preferred);
}

#[test]
fn fallback_skips_active_or_half_disabled_crtcs_instead_of_stealing_them() {
    for busy in [
        Crtc {
            id: 59,
            framebuffer: Some(80),
            mode: Some(mode()),
            position: (0, 0),
        },
        Crtc {
            id: 59,
            framebuffer: Some(80),
            mode: None,
            position: (0, 0),
        },
        Crtc {
            id: 59,
            framebuffer: None,
            mode: Some(mode()),
            position: (0, 0),
        },
    ] {
        let mut topology = disabled();
        topology.crtcs[0] = busy;

        let error = topology.select().unwrap_err();

        assert!(error.to_string().contains("no unused compatible"));
    }
}

#[test]
fn another_encoder_or_disconnected_connector_reservation_is_not_stolen() {
    for other_encoder in [false, true] {
        let mut topology = disabled();
        if other_encoder {
            topology.encoders.push(Encoder {
                id: 71,
                current_crtc: Some(59),
                possible_crtcs: vec![59],
            });
        } else {
            topology.connectors.push(Connector {
                id: 72,
                connected: false,
                hdmi: false,
                current_encoder: Some(61),
                encoders: vec![61],
                modes: vec![],
            });
        }

        let error = topology.select().unwrap_err();

        assert!(error.to_string().contains("no unused compatible"));
    }
}

#[test]
fn active_cloned_output_is_refused() {
    let mut topology = active();
    topology.connectors.push(Connector {
        id: 72,
        connected: false,
        hdmi: false,
        current_encoder: Some(61),
        encoders: vec![61],
        modes: vec![],
    });

    let error = topology.select().unwrap_err();

    assert!(error.to_string().contains("shared"));
}

#[test]
fn fallback_tries_a_later_advertised_encoder_and_compatible_crtc() {
    let mut topology = disabled();
    topology.encoders[0].possible_crtcs.clear();
    topology.connectors[0].encoders.push(71);
    topology.encoders.push(Encoder {
        id: 71,
        current_crtc: None,
        possible_crtcs: vec![69],
    });
    topology.crtcs.push(Crtc {
        id: 69,
        framebuffer: None,
        mode: None,
        position: (0, 0),
    });

    let output = topology.select().unwrap();

    assert_eq!((output.encoder, output.previous.id), (71, 69));
}

#[test]
fn active_bad_mode_or_partial_state_is_not_silently_replaced() {
    for case in 0..4 {
        let mut topology = active();
        match case {
            0 => topology.crtcs[0].mode.as_mut().unwrap().timing.size = (1920, 1080),
            1 => topology.crtcs[0].mode.as_mut().unwrap().timing.flags |= 1 << 4,
            2 => topology.crtcs[0].mode = None,
            _ => topology.crtcs[0].framebuffer = None,
        }

        assert!(topology.select().is_err(), "case {case}");
    }
}

#[test]
fn unsupported_advertised_modes_are_refused() {
    for case in 0..9 {
        let mut topology = disabled();
        let timing = &mut topology.connectors[0].modes[0].timing;
        match case {
            0 => timing.size = (1920, 1080),
            1 => timing.flags |= 1 << 4,
            2 => timing.flags |= 1 << 5,
            3 => timing.clock = 0,
            4 => timing.vscan = 2,
            5 => timing.horizontal.0 = 1200,
            6 => timing.horizontal.1 = timing.horizontal.0,
            7 => timing.vertical.2 = timing.vertical.1,
            _ => topology.connectors[0].modes.clear(),
        }

        let error = topology.select().unwrap_err();

        assert!(
            error.to_string().contains("does not advertise"),
            "case {case}: {error}"
        );
    }
}

#[test]
fn disconnected_or_ambiguous_outputs_are_refused() {
    for case in 0..2 {
        let mut topology = disabled();
        match case {
            0 => topology.connectors[0].connected = false,
            _ => {
                let mut second = topology.connectors[0].clone();
                second.id = 72;
                topology.connectors.push(second);
            }
        }

        assert!(topology.select().is_err(), "case {case}");
    }
}

#[test]
fn non_hdmi_is_supported() {
    let mut topology = disabled();
    topology.connectors[0].hdmi = false;

    assert!(topology.select().is_ok());
}

#[test]
fn explicit_connector_selection_checks_the_entire_topology() {
    let mut topology = disabled();
    let id = topology.connectors[0].id;
    let mut second = topology.connectors[0].clone();
    second.id = 72;
    topology.connectors.push(second);

    assert!(topology.select().is_err());
    assert_eq!(topology.select_connector(Some(id)).unwrap().connector, id);
    assert!(topology.select_connector(Some(999)).is_err());

    // A selected connector cannot steal an encoder routed by another output.
    topology.connectors[1].current_encoder = Some(topology.encoders[0].id);
    assert!(topology.select_connector(Some(id)).is_err());
}

#[test]
fn inconsistent_or_unbounded_resource_inventory_is_refused() {
    for case in 0..13 {
        let mut topology = disabled();
        match case {
            0 => topology.crtcs[0].id = 0,
            1 => topology.crtcs.push(topology.crtcs[0]),
            2 => topology.connectors[0].encoders.push(100),
            3 => topology.connectors[0].current_encoder = Some(100),
            4 => topology.encoders[0].possible_crtcs.push(100),
            5 => topology.encoders[0].current_crtc = Some(100),
            6 => topology.crtcs[0].framebuffer = Some(0),
            7 => topology.connectors[0].modes = vec![mode(); 129],
            8 => topology.encoders = vec![topology.encoders[0].clone(); 33],
            9 => topology.crtcs.clear(),
            10 => topology.connectors[0].encoders.clear(),
            11 => topology.encoders[0].possible_crtcs.push(59),
            _ => topology.connectors[0].encoders.push(61),
        }

        assert!(topology.select().is_err(), "case {case}");
    }
}

#[test]
fn current_route_must_be_advertised_and_compatible() {
    for case in 0..2 {
        let mut topology = active();
        if case == 0 {
            topology.encoders.push(Encoder {
                id: 71,
                current_crtc: None,
                possible_crtcs: vec![],
            });
            topology.connectors[0].encoders = vec![71];
        } else {
            topology.encoders[0].possible_crtcs.clear();
        }

        assert!(topology.select().is_err(), "case {case}");
    }
}

#[test]
fn disable_restore_uses_null_fb_null_mode_and_no_connectors() {
    let mut topology = disabled();
    topology.crtcs[0].position = (123, 456); // Stale GETCRTC x/y are irrelevant when disabled.
    let output = topology.select().unwrap();

    let request = output.enable_request(90).unwrap();
    let current = Crtc {
        id: request.crtc,
        framebuffer: request.framebuffer,
        position: request.position,
        mode: request.mode,
    };
    output.verify(current, 90).unwrap();
    let restore = output.restore_request(current, 90).unwrap().unwrap();

    assert_eq!(request.connectors, vec![62]);
    assert_eq!(
        restore,
        Request {
            crtc: 59,
            framebuffer: None,
            position: (0, 0),
            connectors: vec![],
            mode: None
        }
    );
    output
        .verify_restored(Crtc {
            id: 59,
            framebuffer: None,
            position: (0, 0),
            mode: None,
        })
        .unwrap();
}

#[test]
fn active_restore_keeps_saved_fb_timing_position_and_connector() {
    let topology = active();
    let output = topology.select().unwrap();
    let current = Crtc {
        id: 59,
        framebuffer: Some(90),
        position: (0, 0),
        mode: Some(output.mode),
    };

    let restore = output.restore_request(current, 90).unwrap().unwrap();

    assert_eq!(
        restore,
        Request {
            crtc: 59,
            framebuffer: Some(80),
            position: (11, 22),
            connectors: vec![62],
            mode: Some(mode())
        }
    );
    output.verify_restored(topology.crtcs[0]).unwrap();
}

#[test]
fn failed_modeset_with_unchanged_state_needs_no_restore_ioctl() {
    for topology in [active(), disabled()] {
        let output = topology.select().unwrap();

        let request = output.restore_request(topology.crtcs[0], 90).unwrap();

        assert_eq!(request, None);
    }
}

#[test]
fn changed_owner_prevents_restoring_either_active_or_disabled_state() {
    for topology in [active(), disabled()] {
        let output = topology.select().unwrap();
        let current = Crtc {
            id: 59,
            framebuffer: Some(100),
            position: (0, 0),
            mode: Some(mode()),
        };

        let error = output.restore_request(current, 90).unwrap_err();

        assert!(error.to_string().contains("another owner"));
    }
}

#[test]
fn readback_checks_identity_fb_position_and_full_timing_but_not_mode_metadata() {
    let output = disabled().select().unwrap();
    let good = Crtc {
        id: 59,
        framebuffer: Some(90),
        position: (0, 0),
        mode: Some(mode()),
    };
    let mut metadata_only = good;
    metadata_only.mode.as_mut().unwrap().metadata = 99;
    metadata_only.mode.as_mut().unwrap().preferred = true;

    output.verify(metadata_only, 90).unwrap();
    for case in 0..8 {
        let mut current = good;
        match case {
            0 => current.id = 60,
            1 => current.framebuffer = Some(91),
            2 => current.position = (1, 0),
            3 => current.mode = None,
            4 => current.mode.as_mut().unwrap().timing.clock += 1,
            5 => current.mode.as_mut().unwrap().timing.horizontal.2 += 1,
            6 => current.mode.as_mut().unwrap().timing.flags ^= 1,
            _ => current.mode.as_mut().unwrap().timing.vscan += 1,
        }
        assert!(output.verify(current, 90).is_err(), "case {case}");
    }
}

#[test]
fn restore_readback_requires_actual_previous_state() {
    for topology in [active(), disabled()] {
        let output = topology.select().unwrap();
        let still_ours = Crtc {
            id: 59,
            framebuffer: Some(90),
            position: (0, 0),
            mode: Some(mode()),
        };

        assert!(output.verify_restored(still_ours).is_err());
    }
}

#[test]
fn invalid_framebuffer_or_crtc_cannot_be_used_in_requests() {
    let output = disabled().select().unwrap();
    let mut wrong_crtc = output.previous;
    wrong_crtc.id = 60;

    assert!(output.enable_request(0).is_err());
    assert!(output.verify(output.previous, 0).is_err());
    assert!(output.restore_request(output.previous, 0).is_err());
    assert!(output.restore_request(wrong_crtc, 90).is_err());
}

#[test]
fn topology_changes_before_modeset_are_refused() {
    let initial = disabled();

    initial.verify_unchanged(&initial).unwrap();
    for case in 0..6 {
        let mut current = initial.clone();
        match case {
            0 => current.connectors[0].connected = false,
            1 => current.connectors[0].current_encoder = Some(61),
            2 => current.encoders[0].current_crtc = Some(59),
            3 => current.encoders[0].possible_crtcs.clear(),
            4 => current.crtcs[0].framebuffer = Some(100),
            _ => current.connectors[0].modes[0].timing.clock += 1,
        }

        let error = initial.verify_unchanged(&current).unwrap_err();

        assert!(
            error.to_string().contains("topology changed"),
            "case {case}"
        );
    }
}
