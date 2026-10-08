// SPDX-License-Identifier: BSD-3-Clause
//! File-backed controller layouts, read once before input acquisition.
use crate::{
    controller_profile::{self, invalid, Identity},
    gamepad::profile::Profile as EvdevProfile,
    hid::Profile as HidProfile,
};
use serde::Deserialize;
use std::{collections::BTreeMap, fmt, fs, io, path::{Path}};

const MAX_ENTRIES: usize = 256;
const MAX_PROFILES: usize = 64;
const MAX_TOTAL_BYTES: usize = 512 * 1024;

#[derive(Default)]
pub struct Profiles {
    pub hid: Vec<HidProfile>,
    pub evdev: Vec<EvdevProfile>,
    pub conflicts: Vec<Conflict>,
    evdev_declared: bool,
}

/// A conflicting JSON match, not a blacklist of controller models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub backend: String,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
    pub count: usize,
}
impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = match (self.count % 100, self.count % 10) {
            (11..=14, _) => "профилей",
            (_, 1) => "профиль",
            (_, 2..=4) => "профиля",
            _ => "профилей",
        };
        write!(f, "для {:04x}:{:04x} найдено {} {} ({}, {:?}); этот backend отключён",
            self.vendor, self.product, self.count, noun, self.backend, self.name)
    }
}

#[derive(Deserialize)]
struct Header {
    backend: String,
    name: String,
    #[serde(rename = "match")]
    identity: Identity,
}

pub fn load(directory: &Path, hid: Option<&Path>, evdev: Option<&Path>) -> io::Result<Profiles> {
    // Explicit files replace their backend, not individual device identities.
    let mut profiles = Profiles {
        hid: hid.map(HidProfile::load).transpose()?.into_iter().collect(),
        evdev: evdev.map(EvdevProfile::load).transpose()?.into_iter().collect(),
        conflicts: Vec::new(),
        evdev_declared: evdev.is_some(),
    };
    if hid.is_some() && evdev.is_some() {
        return Ok(profiles);
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        io::Error::new(error.kind(), format!("controls directory {}: {error}", directory.display()))
    })?;
    let mut files = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid("controls directory exceeds 256 entries"));
        }
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "json") {
            files.push(path);
            if files.len() > MAX_PROFILES {
                return Err(invalid("controls directory exceeds 64 JSON profiles"));
            }
        }
    }
    files.sort();
    let mut total_bytes = 0;
    let mut identities = BTreeMap::new();
    for path in files {
        let mut parse = || -> io::Result<()> {
            let bytes = controller_profile::read(&path)?;
            total_bytes += bytes.len();
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(invalid("controls directory exceeds 512 KiB of JSON"));
            }
            let header: Header = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(format!("invalid profile header: {error}")))?;
            match header.backend.as_str() {
                "hidraw" if hid.is_some() => return Ok(()),
                "evdev" if evdev.is_some() => return Ok(()),
                "hidraw" => profiles.hid.push(HidProfile::parse(&bytes)?),
                "evdev" => profiles.evdev.push(EvdevProfile::parse(&bytes)?),
                _ => return Err(invalid("unsupported input profile backend")),
            }
            let key = (header.backend, header.name, header.identity.bus,
                header.identity.vendor, header.identity.product);
            *identities.entry(key).or_insert(0) += 1;
            Ok(())
        };
        parse().map_err(|error| {
            io::Error::new(error.kind(), format!("input profile {}: {error}", path.display()))
        })?;
    }
    if profiles.hid.is_empty() && profiles.evdev.is_empty() {
        return Err(invalid(format!("controls directory {} has no input profiles", directory.display())));
    }
    profiles.evdev_declared = !profiles.evdev.is_empty();
    profiles.conflicts = identities.into_iter().filter_map(|((backend, name, _, vendor, product), count)| {
        (count > 1).then(|| Conflict {
            backend,
            name,
            vendor: u16::from_str_radix(&vendor, 16).expect("validated profile vendor"),
            product: u16::from_str_radix(&product, 16).expect("validated profile product"),
            count,
        })
    }).collect();
    // Ambiguity belongs to one backend; a unique alternative remains usable.
    profiles.hid.retain(|profile| !profiles.conflicts.iter().any(|conflict| {
        conflict.backend == "hidraw" && profile.matches(3, conflict.vendor, conflict.product, &conflict.name)
    }));
    profiles.evdev.retain(|profile| !profiles.conflicts.iter().any(|conflict| {
        conflict.backend == "evdev" && profile.matches(3, conflict.vendor, conflict.product, &conflict.name)
    }));
    Ok(profiles)
}

/// Probe all declared evdev identities, without a board-specific allowlist.
pub fn load_probe(directory: &Path) -> io::Result<Profiles> {
    let profiles = load(directory, None, None)?;
    if !profiles.evdev_declared {
        return Err(invalid("controls directory has no evdev profiles for input probe"));
    }
    Ok(profiles)
}
