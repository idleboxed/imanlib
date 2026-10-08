// SPDX-License-Identifier: BSD-3-Clause
//! Linux input records; read-only acquisition is optional and explicit.
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub path: String,
    pub name: String,
    pub axis_codes: Vec<u8>,
    pub key_codes: Vec<u16>,
    /// value, minimum, maximum, fuzz, flat, resolution from EVIOCGABS.
    pub abs_info: BTreeMap<u8, [i32; 6]>,
    pub identity: BTreeMap<String, String>,
}

impl DeviceInfo {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.axis_codes.len() > 64
            || self.axis_codes.iter().any(|&code| code >= 64)
            || self.key_codes.len() > 768
            || self.key_codes.iter().any(|&code| code >= 768)
            || self.axis_codes.windows(2).any(|pair| pair[0] >= pair[1])
            || self.key_codes.windows(2).any(|pair| pair[0] >= pair[1])
            || self.abs_info.len() > 64
            || self.abs_info.keys().any(|code| !self.axis_codes.contains(code))
            || self.path.len() > 256
            || self.name.len() > 256
            || self.identity.len() > 12
            || self.identity.iter().any(|(key, value)| key.len() > 64 || value.len() > 1024)
        {
            return Err("diagnostic device metadata exceeds limits or is invalid");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Event {
    /// Linux input_event has two native unsigned-long timestamp words, then
    /// u16/u16/i32. ARM time64 userspace must still read the 16-byte kernel ABI.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if !matches!(bytes.len(), 16 | 24) {
            return None;
        }
        let payload = &bytes[bytes.len() - 8..];
        Some(Self {
            kind: u16::from_ne_bytes(payload[0..2].try_into().ok()?),
            code: u16::from_ne_bytes(payload[2..4].try_into().ok()?),
            value: i32::from_ne_bytes(payload[4..8].try_into().ok()?),
        })
    }
}

/// RA 1.22.2 udev_joypad.c: init_pad() scans four ranges, capped at 64.
/// This is a calculation from EV_KEY capabilities, not a physical button label.
pub fn find_retroarch_button_index(keys: &[u16], code: u16) -> Option<usize> {
    (103..=108)
        .chain(256..767)
        .chain(0..103)
        .chain(109..256)
        .filter(|key| keys.contains(key))
        .take(64)
        .position(|key| key == code)
        .filter(|_| code != 0)
}

#[cfg(all(feature = "input", target_os = "linux"))]
pub mod linux;
