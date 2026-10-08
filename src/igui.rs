// SPDX-License-Identifier: BSD-3-Clause
//! Shared GUI wire types and validation; no session host implementation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Commands forwarded by the session host for the GUI to handle.
pub const CONTROL_COMMANDS: &[&str] = &[
    "igui.status",
    "igui.input",
    "igui.screenshot",
    "igui.close",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterValue<T> {
    #[default]
    All,
    Unknown,
    Value(T),
}

impl<T: PartialEq> FilterValue<T> {
    pub fn matches(&self, value: &Option<T>) -> bool {
        match self {
            Self::All => true,
            Self::Unknown => value.is_none(),
            Self::Value(expected) => value.as_ref() == Some(expected),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiState {
    pub selected_hash: Option<String>,
    pub letter: Option<char>,
    pub platform: Option<String>,
    pub favourites_only: bool,
    pub trash_only: bool,
    pub players: FilterValue<u8>,
    pub genre: FilterValue<String>,
    pub letter_cursor: usize,
    pub platform_cursor: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bootstrap {
    pub diag_display: bool,
    pub diag_input: bool,
    /// Interactive read-only input tester, independent of passive diagnostics.
    #[serde(default)]
    pub input_test: bool,
    pub preferences_writable: bool,
    pub root: PathBuf,
    pub state: GuiState,
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub hash: String,
    pub image: String,
    pub platform: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Handoff {
    pub game: LaunchRequest,
    pub state: GuiState,
}

pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_image_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
}

pub fn valid_image_path(path: &str) -> bool {
    path.len() <= 512 && path.split('/').all(valid_image_name)
}

/// A logical GUI action, not a keyboard scan code or a held button.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRequest {
    pub action: String,
}

/// Omit path for a temporary RAM screenshot; otherwise explicitly save a new file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScreenshotRequest {
    pub path: Option<PathBuf>,
}

/// Metadata for a completed screenshot; image creation remains in the GUI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub path: PathBuf,
    pub temporary: bool,
    pub width: u32,
    pub height: u32,
    pub format: String,
}

/// Acknowledges a requested handoff, not completion or ownership of input devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTestAcknowledgement {
    pub handoff_received: bool,
    pub test_started: bool,
}
