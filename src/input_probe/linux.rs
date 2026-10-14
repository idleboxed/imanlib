// SPDX-License-Identifier: BSD-3-Clause
//! Bounded diagnostic reader; acquisition is shared with normal navigation.
use super::{Probe, MAX_CONNECTIONS, MAX_STREAMS};
use crate::evdev::linux::small_text;
pub use crate::evdev::linux::{DiagnosticSource, Pad, ProfileSource, Source, EVENT_BYTES};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
};

struct Active {
    pad: Box<dyn Pad>,
    dropped: bool,
    packet_complete: bool,
}
pub struct Reader<S: Source> {
    source: S,
    pads: Vec<Option<Active>>,
    next_scan: u64,
    errors: BTreeSet<String>,
}
impl<S: Source> Reader<S> {
    /// Create a bounded probe reader with its source; devices are acquired by polling.
    pub fn new(source: S) -> Self {
        Self {
            source,
            pads: (0..MAX_STREAMS).map(|_| None).collect(),
            next_scan: 0,
            errors: BTreeSet::new(),
        }
    }
    fn error(&mut self, probe: &mut Probe, ms: u64, message: String) {
        if self.errors.len() < 32 && self.errors.insert(message.clone()) {
            probe.records.push(json!({"event":"device_error","ms":ms,"message":message.chars().take(200).collect::<String>()}));
        }
    }
    /// Discover and drain permitted streams within read budgets, updating the supplied probe model.
    pub fn poll(&mut self, probe: &mut Probe, ms: u64) {
        if ms >= self.next_scan && probe.connections < MAX_CONNECTIONS {
            self.next_scan = ms.saturating_add(2000);
            match self.source.discover() {
                Err(error_item) => self.error(probe, ms, format!("discovery: {error_item}")),
                Ok(paths) => {
                    if paths.len() > MAX_STREAMS {
                        self.error(
                            probe,
                            ms,
                            "more than eight matching evdev paths; extra devices not observed"
                                .into(),
                        );
                    }
                    for path in paths.into_iter().take(64) {
                        if probe
                            .streams
                            .iter()
                            .flatten()
                            .any(|stream| stream.info.path == path.to_string_lossy())
                        {
                            continue;
                        }
                        if self.pads.iter().all(Option::is_some) {
                            break;
                        }
                        match self.source.open(&path) {
                            Ok(pad) => match probe.connect(pad.info().clone(), ms) {
                                Ok(slot) => {
                                    self.pads[slot] = Some(Active {
                                        pad,
                                        dropped: false,
                                        packet_complete: true,
                                    })
                                }
                                Err(error_item) => self.error(probe, ms, error_item.into()),
                            },
                            Err(error_item) => self.error(probe, ms, format!("{}: {error_item}", path.display())),
                        }
                    }
                }
            }
        }
        let mut all_drained = true;
        for slot in 0..MAX_STREAMS {
            let Some(active) = &mut self.pads[slot] else {
                continue;
            };
            let mut drained = false;
            let mut failed = None;
            for _ in 0..256 {
                match active.pad.read() {
                    Ok(Some(error_item)) => {
                        if error_item.kind == 0 && error_item.code == 3 {
                            active.dropped = true;
                            active.packet_complete = false;
                            probe.resync(slot, ms, "SYN_DROPPED");
                        } else if error_item.kind == 0 && error_item.code == 0 {
                            active.packet_complete = true;
                            if active.dropped {
                                active.dropped = false;
                                match active.pad.keys_down().and_then(|keys| {
                                    probe.baseline(slot, &keys, ms).map_err(io::Error::other)
                                }) {
                                    Ok(()) => {}
                                    Err(error_item) => {
                                        failed = Some(error_item.to_string());
                                        break;
                                    }
                                }
                            }
                        } else {
                            active.packet_complete = false;
                            if !active.dropped {
                                probe.event(slot, error_item, ms);
                            }
                        }
                    }
                    Ok(None) => {
                        drained = true;
                        if active.dropped || !active.packet_complete {
                            all_drained = false;
                        } else if !probe.ready(slot) {
                            if let Err(error_item) = active.pad.keys_down().and_then(|keys| {
                                probe.baseline(slot, &keys, ms).map_err(io::Error::other)
                            }) {
                                failed = Some(error_item.to_string());
                            }
                        }
                        break;
                    }
                    Err(error_item) if error_item.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error_item) => {
                        failed = Some(error_item.to_string());
                        break;
                    }
                }
            }
            if let Some(reason) = failed {
                probe.disconnect(slot, ms, &reason);
                self.pads[slot] = None;
                all_drained = false;
            } else if !drained {
                all_drained = false;
                probe.incomplete(slot, ms);
            }
        }
        if all_drained {
            probe.sampled(ms);
        } else {
            probe.cancel_sample();
        }
    }
}

/// USB topology only: no serial numbers, configuration changes, or control transfers.
/// Injectable directory permits fixture coverage without touching a USB device.
pub fn usb_inventory(root: &Path) -> io::Result<Value> {
    let mut nodes = Vec::new();
    let mut truncated = false;
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        if index >= 256 {
            truncated = true;
            break;
        }
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(':') || !entry.path().join("idVendor").exists() {
            continue;
        }
        nodes.push((name, entry.path()));
    }
    nodes.sort_by(|first_device, second_device| first_device.0.cmp(&second_device.0));
    if nodes.len() > 16 {
        truncated = true;
        nodes.truncate(16);
    }
    let mut devices = Vec::new();
    for (name, path) in nodes {
        let mut item = BTreeMap::from([("path".to_string(), name)]);
        for field in [
            "idVendor",
            "idProduct",
            "bDeviceClass",
            "speed",
            "devpath",
            "busnum",
            "devnum",
            "maxchild",
        ] {
            item.insert(
                field.into(),
                small_text(&path.join(field)).chars().take(64).collect(),
            );
        }
        devices.push(item);
    }
    Ok(json!({"devices":devices,"truncated":truncated}))
}
