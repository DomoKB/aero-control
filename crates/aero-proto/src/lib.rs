//! JSON messages for the future `\\.\pipe\aero-control` channel.
//!
//! The pipe server itself is not in this crate. Messages are versioned so a
//! newer GUI can be rejected by an older service instead of applying a
//! half-understood curve.

use aero_core::{CurvePoint, FanMode};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub protocol_version: u32,
    pub body: RequestBody,
}

impl Request {
    pub fn new(body: RequestBody) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            body,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RequestBody {
    GetStatus,
    ApplyProfile {
        name: String,
    },
    SetFanMode {
        mode: FanMode,
        fixed_duty_raw: Option<u8>,
    },
    UploadCurve {
        points: Vec<CurvePoint>,
    },
    RestoreAuto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub protocol_version: u32,
    pub ok: bool,
    pub message: String,
}

impl Response {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            ok: true,
            message: message.into(),
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            ok: false,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_with_the_protocol_version() {
        let request = Request::new(RequestBody::SetFanMode {
            mode: FanMode::Quiet,
            fixed_duty_raw: None,
        });
        let json = serde_json::to_string(&request).unwrap();
        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.protocol_version, PROTOCOL_VERSION);
        assert_eq!(parsed, request);
    }
}
