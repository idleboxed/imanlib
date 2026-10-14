// SPDX-License-Identifier: BSD-3-Clause
use super::{invalid, Profile, MAX_DESCRIPTOR_BYTES, MAX_REPORT_BYTES};
use {
    crate::input::Buttons,
    crate::igui::Action,
};
use hidparser::{ReportField, VariableField};
use serde::Deserialize;
use std::io;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum Control {
    South,
    East,
    West,
    North,
    Back,
    Start,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    Dpad,
}
impl Control {
    /// Report whether this logical control uses a button rather than an axis or hat.
    pub(crate) fn is_button(self) -> bool {
        (self as usize) < Self::LeftStickX as usize
    }
}

/// Buttons: 0/1; sticks: -32767..32767; hat: clockwise 0..7 from up, -1 neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State([i32; 17]);
impl Default for State {
    fn default() -> Self {
        let mut value = Self([0; 17]);
        value.0[Control::Dpad as usize] = -1;
        value
    }
}
impl State {
    /// Return the decoded value of one logical HID control.
    pub fn value(self, control: Control) -> i32 {
        self.0[control as usize]
    }
    /// Report whether all buttons, sticks and the directional hat are released.
    pub fn neutral(self) -> bool {
        self.0[..12].iter().all(|variable| *variable == 0)
            && self.0[12..16].iter().all(|variable| variable.abs() <= 16383)
            && self.value(Control::Dpad) == -1
    }
    /// Translate decoded HID controls into logical menu buttons.
    pub fn menu_buttons(self) -> Buttons {
        let mut result = Buttons::default();
        for (control, action) in [
            (Control::West, Action::LowerFilters),
            (Control::East, Action::Favourite),
            (Control::North, Action::Filters),
            (Control::South, Action::Activate),
            (Control::Back, Action::Trash),
            (Control::Start, Action::Start),
        ] {
            result.set(action, self.value(control) != 0);
        }
        let hat = match self.value(Control::Dpad) {
            0 => (0, -1),
            1 => (1, -1),
            2 => (1, 0),
            3 => (1, 1),
            4 => (0, 1),
            5 => (-1, 1),
            6 => (-1, 0),
            7 => (-1, -1),
            _ => (0, 0),
        };
        for (axis, hat, negative, positive) in [
            (Control::LeftStickX, hat.0, Action::Left, Action::Right),
            (Control::LeftStickY, hat.1, Action::Up, Action::Down),
        ] {
            let stick = self.value(axis);
            let direction = if hat != 0 {
                hat
            } else if stick < -16383 {
                -1
            } else if stick > 16383 {
                1
            } else {
                0
            };
            result.set(negative, direction < 0);
            result.set(positive, direction > 0);
        }
        result
    }
}

struct Field {
    control: Control,
    start: u32,
    width: u32,
    min: i64,
    max: i64,
    null: bool,
}
impl Field {
    fn new(control: Control, field_item: &VariableField) -> io::Result<Self> {
        let min = i64::from(i32::from(field_item.logical_minimum));
        let max = if min < 0 {
            i64::from(i32::from(field_item.logical_maximum))
        } else {
            i64::from(u32::from(field_item.logical_maximum))
        };
        let width = field_item
            .bits
            .end
            .checked_sub(field_item.bits.start)
            .filter(|width_item| (1..=32).contains(width_item))
            .ok_or_else(|| invalid("unsupported HID field width"))?;
        let representable = if min < 0 {
            (-(1i64 << (width - 1)), (1i64 << (width - 1)) - 1)
        } else {
            (0, (1i64 << width) - 1)
        };
        if field_item.attributes.constant
            || field_item.attributes.relative
            || field_item.attributes.buffered_bytes
            || field_item.attributes.nonlinear
            || min >= max
            || min < representable.0
            || max > representable.1
            || (control.is_button() && (min, max) != (0, 1))
            || (control == Control::Dpad && max - min != 7)
        {
            return Err(invalid("unsupported HID field attributes/range"));
        }
        Ok(Self {
            control,
            start: field_item.bits.start,
            width,
            min,
            max,
            null: field_item.attributes.null_state,
        })
    }
    fn read(&self, data: &[u8]) -> io::Result<i32> {
        // Offsets come only from the parsed descriptor, never from model-specific code.
        let mut raw = 0i64;
        for bit in 0..self.width {
            let offset = (self.start + bit) as usize;
            raw |= i64::from((data[offset / 8] >> (offset % 8)) & 1) << bit;
        }
        if self.min < 0 && raw & (1 << (self.width - 1)) != 0 {
            raw -= 1 << self.width;
        }
        if !(self.min..=self.max).contains(&raw) {
            if self.control == Control::Dpad && self.null {
                return Ok(-1);
            }
            return Err(invalid("HID value outside logical range"));
        }
        Ok(if self.control.is_button() {
            raw as i32
        } else if self.control == Control::Dpad {
            (raw - self.min) as i32
        } else {
            (((raw - self.min) * 65534 / (self.max - self.min)) - 32767) as i32
        })
    }
}
struct Report {
    id: u8,
    bytes: usize,
    fields: Vec<Field>,
}
pub struct Decoder {
    reports: Vec<Report>,
    numbered: bool,
}
impl Decoder {
    /// Validate a bounded HID descriptor against the selected controller profile.
    pub fn new(profile: &Profile, descriptor: &[u8]) -> io::Result<Self> {
        bounded_descriptor(descriptor)?;
        let parsed = hidparser::parse_report_descriptor(descriptor)
            .map_err(|error| invalid(format!("HID descriptor: {error:?}")))?;
        if !parsed.bad_input_reports.is_empty()
            || !parsed.bad_output_reports.is_empty()
            || !parsed.bad_features.is_empty()
        {
            return Err(invalid("invalid HID reports in descriptor"));
        }
        let numbered = profile.report_ids != [0];
        // Do not silently consume input packets that have no declared channel.
        if parsed.input_reports.len() != profile.report_ids.len() {
            return Err(invalid("HID input reports do not match declared channels"));
        }
        let mut reports = Vec::new();
        for &id in &profile.report_ids {
            let report = parsed
                .input_reports
                .iter()
                .find(|report_item| report_item.report_id.map(u32::from).unwrap_or(0) == u32::from(id))
                .ok_or_else(|| invalid("HID controller report missing"))?;
            let bytes = report.size_in_bits.div_ceil(8);
            if bytes == 0 || bytes + usize::from(numbered) > MAX_REPORT_BYTES {
                return Err(invalid("HID input report too large"));
            }
            let mut fields = Vec::new();
            for (&control, &usage) in &profile.bindings {
                let matches: Vec<_> = report
                    .fields
                    .iter()
                    .filter_map(|field_item| match field_item {
                        ReportField::Variable(variable) if u32::from(variable.usage) == usage => Some(variable),
                        _ => None,
                    })
                    .collect();
                if matches.len() != 1 {
                    return Err(invalid("HID usage missing, array or ambiguous"));
                }
                if matches[0].bits.end as usize > report.size_in_bits {
                    return Err(invalid("HID field exceeds report"));
                }
                fields.push(Field::new(control, matches[0])?);
            }
            reports.push(Report { id, bytes, fields });
        }
        Ok(Self { reports, numbered })
    }
    /// Iterate the input report IDs accepted by this decoder.
    pub fn report_ids(&self) -> impl Iterator<Item = u8> + '_ {
        self.reports.iter().map(|report_item| report_item.id)
    }
    /// Decode one validated input packet and return its report ID and logical control state.
    pub fn decode(&self, packet: &[u8]) -> io::Result<(u8, State)> {
        let (id, data) = if self.numbered {
            let (&id, data) = packet
                .split_first()
                .ok_or_else(|| invalid("empty HID packet"))?;
            (id, data)
        } else {
            (0, packet)
        };
        let report = self
            .reports
            .iter()
            .find(|report_item| report_item.id == id)
            .ok_or_else(|| invalid("unknown HID report ID"))?;
        if data.len() != report.bytes {
            return Err(invalid("incorrect HID report length"));
        }
        let mut state = State::default();
        for field in &report.fields {
            state.0[field.control as usize] = field.read(data)?;
        }
        Ok((id, state))
    }
}

// Bound allocations BEFORE calling the general parser. This is an envelope
// validator, not a second interpretation of usages, offsets or logical ranges.
fn bounded_descriptor(bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(invalid("invalid HID descriptor size"));
    }
    let (mut pos, mut count, mut size, mut fields, mut depth) = (0, 0u32, 0u32, 0u32, 0u32);
    while pos < bytes.len() {
        let tag = bytes[pos];
        pos += 1;
        if tag == 0xfe {
            return Err(invalid("HID long items unsupported"));
        }
        let len = [0, 1, 2, 4][usize::from(tag & 3)];
        let data = bytes
            .get(pos..pos + len)
            .ok_or_else(|| invalid("truncated HID item"))?;
        pos += len;
        let mut value = 0u32;
        for (index, byte) in data.iter().enumerate() {
            value |= u32::from(*byte) << (8 * index);
        }
        match tag & 0xfc {
            0x74 => {
                size = value;
                if size == 0 || size > 32 {
                    return Err(invalid("HID report size limit"));
                }
            }
            0x94 => {
                count = value;
                if count == 0 || count > 64 {
                    return Err(invalid("HID report count limit"));
                }
            }
            0x84 if !(1..=255).contains(&value) => return Err(invalid("invalid HID report ID")),
            0xa4 | 0xb4 | 0xa8 => return Err(invalid("HID Push/Pop/delimiter unsupported")),
            0xa0 => {
                depth += 1;
                if depth > 4 {
                    return Err(invalid("HID collection depth limit"));
                }
            }
            0xc0 => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid("unbalanced HID collection"))?;
            }
            0x80 | 0x90 | 0xb0 => {
                fields += count;
                if size == 0 || count == 0 || fields > 512 {
                    return Err(invalid("HID field budget exceeded"));
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid("unclosed HID collection"));
    }
    Ok(())
}
