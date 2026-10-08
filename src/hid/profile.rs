// SPDX-License-Identifier: BSD-3-Clause
use super::{invalid, Control};
use crate::controller_profile::{self, unique_map, Identity, MAX_BYTES};
use serde::Deserialize;
use std::{collections::BTreeMap, io, path::Path};

#[derive(Clone, Deserialize)]
struct Controller {
    input_report_id: u8,
}

#[derive(Clone, Deserialize)]
struct Document {
    name: String,
    backend: String,
    access: String,
    #[serde(rename = "match")]
    identity: Identity,
    controllers: Vec<Controller>,
    #[serde(deserialize_with = "unique_map")]
    controls: BTreeMap<Control, String>,
}

#[derive(Clone)]
pub struct Profile {
    document: Document,
    pub(crate) bindings: BTreeMap<Control, u32>,
    pub(crate) report_ids: Vec<u8>,
}

impl Profile {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid("HID profile exceeds 32 KiB"));
        }
        let d: Document = serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("invalid HID profile: {e}")))?;
        if d.backend != "hidraw"
            || d.access != "read-only"
            || d.identity.bus != "usb"
        {
            return Err(invalid(
                "unsupported HID profile backend/access/bus",
            ));
        }
        d.identity.validate(&d.name)?;
        let ids: Vec<_> = d.controllers.iter().map(|c| c.input_report_id).collect();
        if ids.is_empty()
            || ids.len() > 8
            || ids
                .iter()
                .any(|id| ids.iter().filter(|v| *v == id).count() != 1)
            || (ids.len() > 1 && ids.contains(&0))
        {
            return Err(invalid("invalid or duplicate HID controller report IDs"));
        }
        let mut bindings = BTreeMap::new();
        for (&control, text) in &d.controls {
            let usage = match text.as_str() {
                "GenericDesktop.X" => 0x10030,
                "GenericDesktop.Y" => 0x10031,
                "GenericDesktop.Z" => 0x10032,
                "GenericDesktop.Rx" => 0x10033,
                "GenericDesktop.Ry" => 0x10034,
                "GenericDesktop.Rz" => 0x10035,
                "GenericDesktop.HatSwitch" => 0x10039,
                _ => {
                    let id: u16 = text
                        .strip_prefix("Button.")
                        .ok_or_else(|| invalid("unknown HID usage"))?
                        .parse()
                        .map_err(|_| invalid("invalid Button usage"))?;
                    if id == 0 {
                        return Err(invalid("Button usage must be positive"));
                    }
                    0x90000 | u32::from(id)
                }
            };
            if control.is_button() != (usage >> 16 == 9)
                || (control == Control::Dpad) != (usage == 0x10039)
                || bindings.values().any(|v| *v == usage)
            {
                return Err(invalid("incompatible or duplicated HID usage binding"));
            }
            bindings.insert(control, usage);
        }
        for c in [Control::South, Control::East, Control::West, Control::North] {
            if !bindings.contains_key(&c) {
                return Err(invalid("missing face button binding"));
            }
        }
        for (x, y) in [
            (Control::LeftStickX, Control::LeftStickY),
            (Control::RightStickX, Control::RightStickY),
        ] {
            if bindings.contains_key(&x) != bindings.contains_key(&y) {
                return Err(invalid("incomplete stick binding"));
            }
        }
        if !bindings.contains_key(&Control::Dpad) && !bindings.contains_key(&Control::LeftStickX) {
            return Err(invalid("HID profile has no navigation controls"));
        }
        Ok(Self {
            document: d,
            bindings,
            report_ids: ids,
        })
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        controller_profile::read(path).and_then(|bytes| Self::parse(&bytes))
            .map_err(|error| io::Error::new(error.kind(), format!("input profile {}: {error}", path.display())))
    }
    pub fn matches(&self, bus: u32, vendor: u16, product: u16, name: &str) -> bool {
        self.document.identity.matches(bus, vendor, product) && name == self.document.name
    }
}
