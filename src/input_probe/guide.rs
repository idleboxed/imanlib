// SPDX-License-Identifier: BSD-3-Clause
//! User-paced observations, not an automatically installed controller mapping.
use serde::Serialize;
use serde_json::{json, Value};

pub const ACTIONS: [&str; 9] = ["Up", "Down", "Left", "Right", "X", "B", "Y", "A", "Select"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    Prepare,
    Player { player: u8 },
    Button { player: u8, action: &'static str },
    Together,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Key {
    pub generation: u32,
    pub code: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub player: u8,
    pub action: &'static str,
    pub key: Option<Key>,
}

#[derive(Default)]
pub struct Guide {
    pub operator: Option<Key>,
    pub candidate: Option<Key>,
    pub ambiguous: bool,
    pub notice: &'static str,
    pub players: [Option<u32>; 4],
    /// At most nine entries per player, retained even if detail logs fill up.
    pub observations: Vec<Observation>,
    first_start: Option<Key>,
}

impl Guide {
    pub fn clear_candidate(&mut self) {
        self.candidate = None;
        self.ambiguous = false;
        self.notice = "";
        self.first_start = None;
    }

    pub fn disconnected(&mut self, generation: u32) -> bool {
        self.clear_candidate();
        if self.operator.is_some_and(|k| k.generation == generation) {
            self.operator = None;
            self.players = [None; 4];
            self.observations.clear();
            return true;
        }
        false
    }

    /// Taps must come from a complete, drained sample. A confirmation in the
    /// same sample as a candidate is refused: cross-device read order is not time.
    pub fn sample(&mut self, stage: Stage, taps: &[Key]) -> (Stage, Vec<Value>) {
        let mut records = Vec::new();
        if stage == Stage::Prepare {
            if taps.len() != 1 {
                if !taps.is_empty() {
                    self.first_start = None;
                    self.notice = "Нажимайте START по одному разу, с отпусканием.";
                }
                return (stage, records);
            }
            let key = taps[0];
            if self.first_start != Some(key) {
                self.first_start = Some(key);
                self.notice = "Ещё раз нажмите и отпустите ту же кнопку START.";
                return (stage, records);
            }
            self.operator = Some(key);
            self.clear_candidate();
            records.push(json!({"event":"operator_key","key":key,"physical_label":"Start","label_is_user_supplied":true}));
            return (Stage::Player { player: 1 }, records);
        }
        if stage == Stage::Finish {
            return (stage, records);
        }
        let confirm = taps.iter().any(|k| Some(*k) == self.operator);
        let other: Vec<_> = taps.iter().filter(|k| Some(**k) != self.operator).collect();
        for &&key in &other {
            if matches!(stage, Stage::Player { .. } | Stage::Button { .. }) {
                if self.candidate.is_some_and(|old| old != key) {
                    self.ambiguous = true;
                } else {
                    self.candidate = Some(key);
                }
            }
        }
        if !confirm {
            return (stage, records);
        }
        if !other.is_empty() {
            self.notice = "Сначала отпустите проверяемую кнопку, затем отдельно START.";
            return (stage, records);
        }
        if self.ambiguous {
            records.push(json!({"event":"step_retry","stage":stage,"reason":"multiple_keys"}));
            self.clear_candidate();
            self.notice = "Было несколько кнопок. Повторите только указанную.";
            return (stage, records);
        }
        let next = match stage {
            Stage::Player { player } => {
                if let Some(key) = self.candidate {
                    if self.players.iter().flatten().any(|&g| g == key.generation) {
                        self.clear_candidate();
                        self.notice = "Этот поток уже выбран. Возьмите другой геймпад.";
                        return (stage, records);
                    }
                    self.players[usize::from(player - 1)] = Some(key.generation);
                    self.observations.retain(|o| o.player != player);
                    records.push(json!({"event":"player_selected","player":player,"generation":key.generation,"operator_confirmed":true}));
                    Stage::Button {
                        player,
                        action: ACTIONS[0],
                    }
                } else {
                    records.push(json!({"event":"player_skipped","player":player}));
                    next_player(player)
                }
            }
            Stage::Button { player, action } => {
                if self
                    .candidate
                    .is_some_and(|k| Some(k.generation) != self.players[usize::from(player - 1)])
                {
                    self.clear_candidate();
                    self.notice = "Нажат другой поток. Повторите нужным геймпадом.";
                    return (stage, records);
                }
                records.push(json!({"event":"step_result","player":player,"action":action,"key":self.candidate,"skipped":self.candidate.is_none(),"operator_confirmed":true,"mapping_written":false}));
                self.observations
                    .retain(|o| o.player != player || o.action != action);
                self.observations.push(Observation {
                    player,
                    action,
                    key: self.candidate,
                });
                let index = ACTIONS.iter().position(|&a| a == action).unwrap();
                if index + 1 < ACTIONS.len() {
                    Stage::Button {
                        player,
                        action: ACTIONS[index + 1],
                    }
                } else {
                    next_player(player)
                }
            }
            Stage::Together => Stage::Finish,
            _ => stage,
        };
        self.clear_candidate();
        (next, records)
    }
}

fn next_player(player: u8) -> Stage {
    if player < 4 {
        Stage::Player { player: player + 1 }
    } else {
        Stage::Together
    }
}
