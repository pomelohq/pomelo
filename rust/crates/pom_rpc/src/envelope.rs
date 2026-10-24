//! JSON bodies of the control frames. Every struct defaults its fields so an older reader accepts a newer
//! writer's extra or missing fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;

/// Bumped only for a wire change an older peer cannot read.
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest protocol this side still speaks. Two peers talk when each one's newest protocol is at least the
/// other's oldest, and then speak the lower of their newest.
pub const MIN_PROTOCOL: u32 = 1;

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
    /// The newest protocol this side speaks; in a server's reply, the one both sides speak.
    pub protocol: u32,
    pub pom_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    pub features: Vec<String>,
    /// The oldest protocol this side speaks; 0 from a peer older than the field, which speaks only `protocol`.
    pub min_protocol: u32,
}

impl Hello {
    pub fn new(pom_version: impl Into<String>) -> Self {
        Hello {
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL,
            pom_version: pom_version.into(),
            ..Hello::default()
        }
    }

    pub fn oldest_protocol(&self) -> u32 {
        match self.min_protocol {
            0 => self.protocol,
            oldest => oldest.min(self.protocol),
        }
    }
}

/// The protocol this side and `peer` both speak, or the refusal that says which side is too old.
pub fn agreed_protocol(peer: &Hello) -> Result<u32, String> {
    if peer.protocol >= MIN_PROTOCOL && peer.oldest_protocol() <= PROTOCOL_VERSION {
        return Ok(peer.protocol.min(PROTOCOL_VERSION));
    }
    Err(format!(
        "{}: peer speaks protocol {} (oldest {}), this side {PROTOCOL_VERSION} (oldest {MIN_PROTOCOL})",
        error_code::PROTOCOL_MISMATCH,
        peer.protocol,
        peer.oldest_protocol()
    ))
}

/// Who refused a handshake as too old or too new, read from a `protocol_mismatch` refusal: the newest protocol
/// of the side that refused (`this_side`) and of the side it refused (`peer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub peer: u32,
    pub this_side: u32,
}

impl Mismatch {
    pub fn parse(message: &str) -> Option<Mismatch> {
        let rest = message.strip_prefix(error_code::PROTOCOL_MISMATCH)?;
        let number_after = |label: &str| -> Option<u32> {
            let (_, after) = rest.split_once(label)?;
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        };
        Some(Mismatch {
            peer: number_after("peer speaks protocol ")?,
            this_side: number_after("this side ")?,
        })
    }

    /// The other side's newest protocol, whichever side refused: the one of the two that is not ours.
    pub fn other_than(&self, ours: u32) -> u32 {
        if self.peer == ours {
            self.this_side
        } else {
            self.peer
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_of_either_format_says_both_protocols() {
        let before = "protocol_mismatch: peer speaks protocol 2, this side 1";
        let now = "protocol_mismatch: peer speaks protocol 1 (oldest 1), this side 3 (oldest 2)";
        assert_eq!(
            Mismatch::parse(before),
            Some(Mismatch {
                peer: 2,
                this_side: 1
            })
        );
        assert_eq!(
            Mismatch::parse(now).map(|found| found.other_than(1)),
            Some(3)
        );
        assert_eq!(Mismatch::parse("protocol_error: bad frame"), None);
    }

    #[test]
    fn a_hello_from_before_min_protocol_speaks_only_its_own() {
        let old: Hello =
            serde_json::from_str(r#"{"protocol":1,"pom_version":"0.9.9"}"#).unwrap_or_default();
        assert_eq!(old.oldest_protocol(), 1);
        assert_eq!(agreed_protocol(&old), Ok(1));
        let too_old = Hello {
            protocol: 0,
            ..Hello::default()
        };
        assert!(agreed_protocol(&too_old).is_err());
    }
}
