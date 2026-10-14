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
    /// Set or clear one logical button in this snapshot.
    pub fn set(&mut self, action: Action, down: bool) {
        let bit = 1 << action as u8;
        if down {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
    }

    /// Report whether a particular logical button is held.
    pub fn down(self, action: Action) -> bool {
        self.0 & (1 << action as u8) != 0
    }

    /// Report whether at least one logical button is held.
    pub fn any(self) -> bool {
        self.0 != 0
    }

    /// Return only buttons held in both snapshots.
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Return buttons held in either snapshot.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
