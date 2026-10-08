// SPDX-License-Identifier: BSD-3-Clause
use {
    crate::controller_profile::self,
    crate::controller_profile::invalid,
    crate::controller_profile::unique_map,
    crate::controller_profile::Identity,
    crate::controller_profile::MAX_BYTES,
    crate::igui::Action,
};
use crate::evdev::DeviceInfo;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Button {
    West,
    East,
    North,
    South,
    Back,
    Start,
    Up,
    Down,
    Left,
    Right,
}
impl Button {
    fn action(self) -> Action {
        match self {
            Self::West => Action::LowerFilters,
            Self::East => Action::Favourite,
            Self::North => Action::Filters,
            Self::South => Action::Activate,
            Self::Back => Action::Trash,
            Self::Start => Action::Start,
            Self::Up => Action::Up,
            Self::Down => Action::Down,
            Self::Left => Action::Left,
            Self::Right => Action::Right,
        }
    }
}
#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
pub(crate) enum AxisBinding {
    Code(u8),
    Directed { code: u8, #[serde(default)] invert: bool },
}
impl AxisBinding {
    pub(crate) fn code(self) -> u8 {
        match self { Self::Code(code) | Self::Directed { code, .. } => code }
    }
    pub(crate) fn invert(self) -> bool {
        matches!(self, Self::Directed { invert: true, .. })
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Axes {
    left_stick_x: Option<AxisBinding>,
    left_stick_y: Option<AxisBinding>,
    dpad_x: Option<AxisBinding>,
    dpad_y: Option<AxisBinding>,
}
#[derive(Clone, Deserialize)]
pub struct Profile {
    name: String,
    backend: String,
    access: String,
    #[serde(rename = "match")]
    identity: Identity,
    #[serde(deserialize_with = "unique_map")]
    buttons: BTreeMap<Button, u16>,
    axes: Axes,
}
impl Profile {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid("input profile exceeds 32 KiB"));
        }
        let profile: Self = serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("invalid evdev profile: {e}")))?;
        profile.identity.validate(&profile.name)?;
        if profile.backend != "evdev" || profile.access != "read-only" {
            return Err(invalid("unsupported evdev profile backend/access"));
        }
        if profile.buttons.values().any(|code| *code >= 768)
            || profile.buttons.values().collect::<BTreeSet<_>>().len() != profile.buttons.len()
            || [Button::West, Button::East, Button::North, Button::South]
                .iter()
                .any(|key| !profile.buttons.contains_key(key))
        {
            return Err(invalid("invalid/missing evdev button binding"));
        }
        let directions = [Button::Up, Button::Down, Button::Left, Button::Right]
            .iter()
            .filter(|k| profile.buttons.contains_key(k))
            .count();
        if directions != 0 && directions != 4 {
            return Err(invalid("incomplete evdev dpad keys"));
        }
        let axes: Vec<_> = profile.axis_bindings().into_iter().flatten().map(AxisBinding::code).collect();
        if axes.iter().any(|v| *v >= 64) || axes.iter().collect::<BTreeSet<_>>().len() != axes.len() {
            return Err(invalid("invalid/duplicate evdev axes"));
        }
        if profile.axes.left_stick_x.is_some() != profile.axes.left_stick_y.is_some()
            || profile.axes.dpad_x.is_some() != profile.axes.dpad_y.is_some() {
            return Err(invalid("incomplete evdev axis pair"));
        }
        if directions == 0 && axes.is_empty() {
            return Err(invalid("missing evdev navigation controls"));
        }
        Ok(profile)
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        controller_profile::read(path).and_then(|bytes| Self::parse(&bytes))
            .map_err(|error| io::Error::new(error.kind(), format!("input profile {}: {error}", path.display())))
    }
    pub fn matches(&self, bus: u32, vendor: u16, product: u16, name: &str) -> bool {
        self.identity.matches(bus, vendor, product) && self.name == name
    }
    pub(crate) fn matches_info(&self, info: &DeviceInfo) -> bool {
        let id = |key: &str| {
            info.identity
                .get(key)
                .and_then(|v| u16::from_str_radix(v, 16).ok())
        };
        match (id("id/bustype"), id("id/vendor"), id("id/product")) {
            (Some(bus), Some(vendor), Some(product)) => {
                self.matches(u32::from(bus), vendor, product, &info.name)
            }
            _ => false,
        }
    }
    pub(crate) fn key_map(&self) -> BTreeMap<u16, Action> {
        self.buttons
            .iter()
            .map(|(b, code)| (*code, b.action()))
            .collect()
    }
    pub(crate) fn axis_bindings(&self) -> [Option<AxisBinding>; 4] {
        [
            self.axes.left_stick_x,
            self.axes.left_stick_y,
            self.axes.dpad_x,
            self.axes.dpad_y,
        ]
    }
}
