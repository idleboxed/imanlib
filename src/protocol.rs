// SPDX-License-Identifier: BSD-3-Clause
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const REQUEST_LIMIT: usize = 16 * 1024;
pub const RESPONSE_LIMIT: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub command: String,
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, command: &str, params: Value) -> Self {
        Self {
            id,
            command: command.into(),
            params,
        }
    }
    pub fn valid(&self) -> bool {
        let mut parts = self.command.split('.');
        let valid = |p: &str| {
            !p.is_empty() && p.len() <= 48 && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        };
        self.id != 0
            && parts.next().is_some_and(valid)
            && parts.next().is_some_and(valid)
            && parts.next().is_none()
            && self.params.is_object()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}
impl RpcError {
    pub fn new(code: &str, message: impl ToString) -> Self {
        Self {
            code: code.into(),
            message: message.to_string().chars().take(512).collect(),
        }
    }
}
impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for RpcError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Done { result: Value },
    Accepted { operation_id: u64 },
    Error { error: RpcError },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}
impl Response {
    pub fn done(id: u64, result: Value) -> Self {
        Self {
            id,
            outcome: Outcome::Done { result },
        }
    }
    pub fn error(id: u64, code: &str, message: impl ToString) -> Self {
        Self {
            id,
            outcome: Outcome::Error {
                error: RpcError::new(code, message),
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event {
        event: String,
        params: Value,
    },
}
