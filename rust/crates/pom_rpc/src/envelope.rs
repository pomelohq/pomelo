//! JSON bodies of the control frames. Every struct defaults its fields so an older reader accepts a newer
//! writer's extra or missing fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;

/// Bumped only for a wire change an older peer cannot read; peers with different values refuse each other.
pub const PROTOCOL_VERSION: u32 = 1;

pub mod error_code {
    pub const PROTOCOL_ERROR: &str = "protocol_error";
    pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
    pub const CANCELLED: &str = "cancelled";
    pub const METHOD_NOT_FOUND: &str = "method_not_found";
    pub const INVALID_PARAMS: &str = "invalid_params";
    pub const INTERNAL: &str = "internal";
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub protocol: u32,
    pub pom_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    pub features: Vec<String>,
}

impl Hello {
    pub fn new(pom_version: impl Into<String>) -> Self {
        Hello {
            protocol: PROTOCOL_VERSION,
            pom_version: pom_version.into(),
            ..Hello::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
}

impl RemoteError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        RemoteError {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Exactly one of `ok` and `err` is set on the wire; a missing `ok` without `err` reads as `null`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<RemoteError>,
}

impl Response {
    pub fn from_result(result: Result<Value, RemoteError>) -> Self {
        match result {
            Ok(value) => Response {
                ok: Some(value),
                err: None,
            },
            Err(error) => Response {
                ok: None,
                err: Some(error),
            },
        }
    }

    pub fn into_result(self) -> Result<Value, RemoteError> {
        match self.err {
            Some(error) => Err(error),
            None => Ok(self.ok.unwrap_or(Value::Null)),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Push {
    pub topic: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct End {
    pub reason: String,
}

pub fn decode<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(payload).map_err(|error| Error::Protocol(format!("bad json: {error}")))
}
