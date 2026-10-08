// SPDX-License-Identifier: BSD-3-Clause
//! Device-independent D-pad / X / B / Y / A / Select / Start input.
use crate::igui::Action;

pub const INPUT_DIR: &str = "/dev/input";

pub const ACTIONS: [Action; crate::input_metrics::LOGICAL_EDGE_COUNT] = [
    Action::Up,
    Action::Down,
    Action::Left,
    Action::Right,
    // Process a focus shortcut before confirmation pressed in the same frame.
    Action::Filters,
    Action::LowerFilters,
    Action::Activate,
    Action::Favourite,
    Action::Trash,
    Action::Start,
];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Buttons(u16);

impl Buttons {
    pub fn set(&mut self, action: Action, down: bool) {
        let bit = 1 << action as u8;
        if down {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    pub fn down(self, action: Action) -> bool {
        self.0 & (1 << action as u8) != 0
    }

    pub fn any(self) -> bool {
        self.0 != 0
    }

    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
