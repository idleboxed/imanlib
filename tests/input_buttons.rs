// SPDX-License-Identifier: BSD-3-Clause
use imanlib::{igui::Action, input::{Buttons, ACTIONS}};

#[test]
fn independent_logical_buttons_can_be_combined_filtered_and_released() {
    let mut held = Buttons::default();
    let mut others = Buttons::default();
    held.set(Action::Up, true);
    held.set(Action::Activate, true);
    others.set(Action::Activate, true);
    others.set(Action::Start, true);

    let combined = held.union(others);
    let common = held.intersect(others);
    held.set(Action::Up, false);

    assert_eq!(ACTIONS.map(|action| common.down(action)), ACTIONS.map(|action| action == Action::Activate));
    assert!(combined.down(Action::Up));
    assert!(combined.down(Action::Activate));
    assert!(combined.down(Action::Start));
    assert!(!held.down(Action::Up));
    assert!(held.down(Action::Activate));
    assert!(!Buttons::default().any());
}
