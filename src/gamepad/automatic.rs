// SPDX-License-Identifier: BSD-3-Clause
//! One menu owner across read-only HID and evdev sources. Backend selection is
//! separate from emulator ports and does not infer radio connection state.
use super::receiver::{belongs_to_hid_generation, Receiver};
use crate::{
    evdev::linux::Source as EvdevSource,
    hid::receiver::{Receivers, Source as HidSource},
    input::Buttons,
};
use {
    crate::input_metrics::Channel,
    crate::input_metrics::Sample,
    crate::input_metrics::CHANNEL_LIMIT,
};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Hid,
    Evdev,
}

/// Pure construction. Polling preserves the per-backend limits: eight HID and
/// eight evdev FDs, 256 reads per FD, discovery no more than once per two seconds.
pub struct Automatic<E: EvdevSource, H: HidSource> {
    evdev: Receiver<E>,
    hid: Option<Receivers<H>>,
    owner: Option<Backend>,
}
impl<E: EvdevSource, H: HidSource> Automatic<E, H> {
    /// Combine supplied evdev and optional HID receivers without polling or opening devices.
    pub fn new(evdev: Receiver<E>, hid: Option<Receivers<H>>) -> Self {
        Self { evdev, hid, owner: None }
    }
    /// Return the backend currently owning menu input, if any.
    pub fn owner(&self) -> Option<Backend> {
        self.owner
    }
    /// Enable bounded counters in the underlying receivers.
    pub fn enable_diagnostics(&mut self) {
        self.evdev.enable_diagnostics();
        if let Some(hid) = &mut self.hid {
            hid.enable_diagnostics();
        }
    }
    /// Updates only backend counters, connection metadata and global owner flags.
    /// Logical edges/polls are maintained by Gamepad; this method performs no I/O.
    pub fn update_diagnostics(&self, sample: &mut Sample) {
        sample.evdev = self.evdev.diagnostics();
        sample.hid = self.hid.as_ref().map(|hid| hid.diagnostics()).unwrap_or_default();
        sample.evdev_connected = self.evdev.connected();
        sample.evdev_identity = (self.owner == Some(Backend::Evdev))
            .then(|| self.evdev.diagnostic_identity()).flatten();
        let channels = self.hid.as_ref().map(|hid| hid.snapshots()).unwrap_or_default();
        let hid_owner = (self.owner == Some(Backend::Hid))
            .then(|| self.hid.as_ref().and_then(|hid| hid.owner())).flatten();
        sample.channels_omitted = channels.len().saturating_sub(CHANNEL_LIMIT);
        sample.channels = channels.into_iter().take(CHANNEL_LIMIT).map(|channel| Channel {
            generation: channel.id.generation,
            report_id: channel.id.report_id,
            received: channel.received,
            ready: channel.ready,
            owner: hid_owner == Some(channel.id),
        }).collect();
    }
    /// Poll both backends and select one neutral-to-active input owner without alias takeover.
    pub fn poll(&mut self, now: Duration) -> Buttons {
        // A newly available HID alias must not evict an already active evdev
        // representation merely by opening. Its reports are drained but blocked.
        let preserved_evdev = (self.owner == Some(Backend::Evdev))
            .then(|| self.evdev.owner_info()).flatten();
        let hid_buttons = self.hid.as_mut().map(|hid| {
            hid.poll_with_blocked(now, |info| {
                preserved_evdev.is_some_and(|evdev| belongs_to_hid_generation(evdev, &info.generation))
            })
        }).unwrap_or_default();
        let hid_request = self.hid.as_ref().is_some_and(|hid| hid.fresh_request());
        let hid_generations: Vec<PathBuf> = self.hid.as_ref().map(|hid| {
            hid.generations().filter(|generation| {
                !preserved_evdev.is_some_and(|evdev| belongs_to_hid_generation(evdev, generation))
            }).map(|generation| generation.to_path_buf()).collect()
        }).unwrap_or_default();
        let evdev_buttons = self.evdev.poll_with_exclusions(now, &hid_generations);

        // A simultaneous request is resolved once, HID before evdev. Neither
        // backend's state is ORed with the other, including during a handoff.
        let requested = if hid_request {
            Some(Backend::Hid)
        } else if self.evdev.fresh_request() {
            Some(Backend::Evdev)
        } else {
            None
        };
        if let Some(requested) = requested {
            if self.owner != Some(requested) {
                self.evdev.inhibit_owner();
                if let Some(hid) = &mut self.hid {
                    hid.inhibit_owner();
                }
                self.owner = Some(requested);
                return Buttons::default();
            }
        }
        let owner_present = match self.owner {
            Some(Backend::Evdev) => self.evdev.owner_info().is_some(),
            Some(Backend::Hid) => self.hid.as_ref().is_some_and(|hid| hid.owner().is_some()),
            None => false,
        };
        if !owner_present {
            self.owner = None;
            return Buttons::default();
        }
        match self.owner {
            Some(Backend::Hid) => hid_buttons,
            Some(Backend::Evdev) => evdev_buttons,
            None => Buttons::default(),
        }
    }
}
