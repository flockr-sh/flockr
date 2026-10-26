//! `mobile.v2` message types (`docs/mobile/protocol.md` §8–§10). Pure.
//!
//! Client messages are decoded leniently: unknown fields are ignored and
//! unknown enum strings fall back to `Unknown`. Server messages are built with
//! `serde_json` values or the cell encoder in `cells.rs`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const SUBPROTOCOL: &str = "flockr.mobile.v2";
pub(crate) const PATH: &str = "/mobile/v2";
pub(crate) const VERSION: u32 = 2;

/// Close codes (§7).
pub(crate) mod close {
    pub(crate) const NORMAL: u16 = 1000;
    pub(crate) const TOO_BIG: u16 = 1009;
    pub(crate) const PROTOCOL_ERROR: u16 = 4000;
    pub(crate) const UNAUTHORIZED: u16 = 4001;
    pub(crate) const UNSUPPORTED_VERSION: u16 = 4002;
    pub(crate) const REVOKED: u16 = 4003;
    pub(crate) const TOO_MANY_CONNECTIONS: u16 = 4004;
    pub(crate) const PAIRING_FAILED: u16 = 4005;
    pub(crate) const HANDSHAKE_FAILED: u16 = 4006;
    pub(crate) const HELLO_TIMEOUT: u16 = 4008;
    pub(crate) const SERVER_SHUTDOWN: u16 = 4009;
    pub(crate) const RATE_LIMITED: u16 = 4029;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct AppInfo {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct HelloAuth {
    pub device_id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct HelloPair {
    pub code: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub sid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Hello {
    #[serde(default)]
    pub versions: Vec<u32>,
    #[serde(default)]
    pub app: AppInfo,
    #[serde(default)]
    pub auth: Option<HelloAuth>,
    #[serde(default)]
    pub pair: Option<HelloPair>,
    #[serde(default)]
    pub max_in_flight: Option<u32>,
}

impl Hello {
    pub(crate) fn negotiated_version(&self) -> Option<u32> {
        self.versions
            .iter()
            .copied()
            .filter(|version| *version == VERSION)
            .max()
    }

    pub(crate) fn max_in_flight(&self) -> u32 {
        self.max_in_flight.unwrap_or(2).clamp(1, 8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SurfaceMode {
    #[default]
    Observe,
    Fit,
    #[serde(other)]
    Unknown,
}

impl SurfaceMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Observe | Self::Unknown => "observe",
            Self::Fit => "fit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceSubscribe {
    #[serde(default)]
    pub id: Option<String>,
    pub sub: u32,
    pub target: String,
    #[serde(default)]
    pub mode: SurfaceMode,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
    #[serde(default)]
    pub max_fps: Option<u32>,
}

impl SurfaceSubscribe {
    pub(crate) fn max_fps(&self) -> u32 {
        self.max_fps.unwrap_or(30).clamp(1, 60)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceResize {
    #[serde(default)]
    pub id: Option<String>,
    pub sub: u32,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceAck {
    pub sub: u32,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceRef {
    #[serde(default)]
    pub id: Option<String>,
    pub sub: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceScroll {
    #[serde(default)]
    pub id: Option<String>,
    pub sub: u32,
    #[serde(default)]
    pub lines: Option<i64>,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SurfaceHistory {
    pub id: String,
    pub sub: u32,
    #[serde(default)]
    pub lines: Option<u32>,
}

impl SurfaceHistory {
    pub(crate) fn lines(&self) -> u32 {
        self.lines.unwrap_or(500).clamp(1, 10_000)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ApiRequest {
    pub id: String,
    pub method: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct IdOnly {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputKind {
    Text(String),
    Key(Vec<String>),
    Paste(String),
    Submit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Input {
    pub id: Option<String>,
    pub sub: Option<u32>,
    pub target: Option<String>,
    pub kind: InputKind,
}

#[derive(Deserialize)]
struct InputWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    sub: Option<u32>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PushRegister {
    pub id: Option<String>,
    pub expo_token: String,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AppState {
    pub foreground: bool,
}

/// Every client → server message after `hello`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClientMessage {
    Hello(Hello),
    ApiRequest(ApiRequest),
    ApiCancel(IdOnly),
    AgentsWatch(IdOnly),
    AgentsUnwatch(IdOnly),
    SurfaceSubscribe(SurfaceSubscribe),
    SurfaceResize(SurfaceResize),
    SurfaceAck(SurfaceAck),
    SurfaceResync(SurfaceRef),
    SurfaceScroll(SurfaceScroll),
    SurfaceHistory(SurfaceHistory),
    SurfaceUnsubscribe(SurfaceRef),
    Input(Input),
    PushRegister(PushRegister),
    PushUnregister {
        id: Option<String>,
    },
    AppState(AppState),
    DeviceForget {
        id: Option<String>,
    },
    Ping {
        ts: Value,
    },
    /// Unknown `t`; `id` when it had one.
    Unknown {
        t: String,
        id: Option<String>,
    },
}

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodeError {
    /// Not a JSON object with a string `t`: protocol error (close 4000).
    Malformed(String),
    /// Known `t` with a bad shape: answer `bad_request` (with `id` if any).
    BadRequest { id: Option<String>, message: String },
}

fn id_of(value: &Value) -> Option<String> {
    value.get("id").and_then(Value::as_str).map(str::to_owned)
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, DecodeError> {
    let id = id_of(&value);
    serde_json::from_value(value).map_err(|err| DecodeError::BadRequest {
        id,
        message: err.to_string(),
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64
}

impl ClientMessage {
    pub(crate) fn decode(text: &str) -> Result<Self, DecodeError> {
        let value: Value =
            serde_json::from_str(text).map_err(|err| DecodeError::Malformed(err.to_string()))?;
        let Some(t) = value.get("t").and_then(Value::as_str).map(str::to_owned) else {
            return Err(DecodeError::Malformed(
                "message is not an object with a string `t`".to_owned(),
            ));
        };
        if let Some(id) = value.get("id") {
            let ok = id.as_str().is_some_and(valid_id);
            if !ok {
                return Err(DecodeError::BadRequest {
                    id: None,
                    message: "`id` must be a string of 1..=64 characters".to_owned(),
                });
            }
        }
        Ok(match t.as_str() {
            "hello" => Self::Hello(typed(value)?),
            "api.request" => Self::ApiRequest(typed(value)?),
            "api.cancel" => Self::ApiCancel(typed(value)?),
            "agents.watch" => Self::AgentsWatch(typed(value)?),
            "agents.unwatch" => Self::AgentsUnwatch(typed(value)?),
            "surface.subscribe" => {
                let subscribe: SurfaceSubscribe = typed(value)?;
                if subscribe.sub == 0 || subscribe.sub > (1 << 31) {
                    return Err(DecodeError::BadRequest {
                        id: subscribe.id,
                        message: "`sub` must be 1..=2^31".to_owned(),
                    });
                }
                Self::SurfaceSubscribe(subscribe)
            }
            "surface.resize" => Self::SurfaceResize(typed(value)?),
            "surface.ack" => Self::SurfaceAck(typed(value)?),
            "surface.resync" => Self::SurfaceResync(typed(value)?),
            "surface.scroll" => Self::SurfaceScroll(typed(value)?),
            "surface.history" => Self::SurfaceHistory(typed(value)?),
            "surface.unsubscribe" => Self::SurfaceUnsubscribe(typed(value)?),
            "input.text" | "input.key" | "input.paste" | "input.submit" => {
                let id = id_of(&value);
                let wire: InputWire = typed(value)?;
                let bad = |message: &str| DecodeError::BadRequest {
                    id: id.clone(),
                    message: message.to_owned(),
                };
                if wire.sub.is_some() == wire.target.is_some() {
                    return Err(bad("exactly one of `sub` or `target` is required"));
                }
                let kind = match t.as_str() {
                    "input.key" => {
                        let keys = wire.keys.ok_or_else(|| bad("`keys` is required"))?;
                        if keys.is_empty() {
                            return Err(bad("`keys` is empty"));
                        }
                        InputKind::Key(keys)
                    }
                    other => {
                        let text = wire.text.ok_or_else(|| bad("`text` is required"))?;
                        match other {
                            "input.text" => InputKind::Text(text),
                            "input.paste" => InputKind::Paste(text),
                            _ => InputKind::Submit(text),
                        }
                    }
                };
                Self::Input(Input {
                    id: wire.id,
                    sub: wire.sub,
                    target: wire.target,
                    kind,
                })
            }
            "push.register" => Self::PushRegister(typed(value)?),
            "push.unregister" => Self::PushUnregister { id: id_of(&value) },
            "app.state" => Self::AppState(typed(value)?),
            "device.forget" => Self::DeviceForget { id: id_of(&value) },
            "ping" => Self::Ping {
                ts: value.get("ts").cloned().unwrap_or(Value::Null),
            },
            _ => Self::Unknown {
                id: id_of(&value),
                t,
            },
        })
    }
}

/// Whether `token` looks like an Expo push token.
pub(crate) fn valid_expo_token(token: &str) -> bool {
    let inner = token
        .strip_prefix("ExponentPushToken[")
        .or_else(|| token.strip_prefix("ExpoPushToken["))
        .and_then(|rest| rest.strip_suffix(']'));
    inner.is_some_and(|inner| {
        (1..=256).contains(&inner.len())
            && inner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

/// Error codes (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    BadRequest,
    UnknownType,
    Unauthorized,
    PairingFailed,
    TooManyDevices,
    UnsupportedVersion,
    Revoked,
    TooManyConnections,
    RateLimited,
    MethodNotAllowed,
    NotFound,
    PaneBusy,
    TooManySubscriptions,
    PayloadTooLarge,
    PushUnavailable,
    ServerUnavailable,
    Internal,
}

impl ErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::UnknownType => "unknown_type",
            Self::Unauthorized => "unauthorized",
            Self::PairingFailed => "pairing_failed",
            Self::TooManyDevices => "too_many_devices",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Revoked => "revoked",
            Self::TooManyConnections => "too_many_connections",
            Self::RateLimited => "rate_limited",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::NotFound => "not_found",
            Self::PaneBusy => "pane_busy",
            Self::TooManySubscriptions => "too_many_subscriptions",
            Self::PayloadTooLarge => "payload_too_large",
            Self::PushUnavailable => "push_unavailable",
            Self::ServerUnavailable => "server_unavailable",
            Self::Internal => "internal",
        }
    }
}

pub(crate) fn error(id: Option<&str>, code: ErrorCode, message: impl Into<String>) -> String {
    let mut value = json!({ "t": "error", "code": code.as_str(), "message": message.into() });
    if let Some(id) = id {
        value["id"] = Value::String(id.to_owned());
    }
    value.to_string()
}

pub(crate) fn reply(t: &str, id: &str) -> String {
    json!({ "t": t, "id": id }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/mobile-v2")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn decodes_every_client_fixture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mobile-v2");
        let mut decoded = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if !name.starts_with("client.") {
                continue;
            }
            let message = ClientMessage::decode(&fixture(&name))
                .unwrap_or_else(|err| panic!("{name}: {err:?}"));
            assert!(
                !matches!(message, ClientMessage::Unknown { .. }),
                "{name} decoded as unknown"
            );
            decoded += 1;
        }
        assert!(decoded >= 20, "only {decoded} client fixtures");
    }

    #[test]
    fn hello_fixtures_carry_auth_or_pair() {
        let ClientMessage::Hello(hello) =
            ClientMessage::decode(&fixture("client.hello.auth.json")).unwrap()
        else {
            panic!("not hello");
        };
        assert_eq!(hello.negotiated_version(), Some(2));
        assert_eq!(hello.max_in_flight(), 2);
        assert_eq!(hello.auth.unwrap().device_id, "d_q8Zt3mVfR1xKp0aB");
        let ClientMessage::Hello(hello) =
            ClientMessage::decode(&fixture("client.hello.pair.json")).unwrap()
        else {
            panic!("not hello");
        };
        assert!(hello.auth.is_none());
        assert_eq!(hello.pair.unwrap().sid, "Xb3k9QmT0aLz7Rwe2YvN1c");
    }

    #[test]
    fn malformed_and_unknown_messages() {
        assert!(matches!(
            ClientMessage::decode("not json"),
            Err(DecodeError::Malformed(_))
        ));
        assert!(matches!(
            ClientMessage::decode(r#"{"x":1}"#),
            Err(DecodeError::Malformed(_))
        ));
        assert!(matches!(
            ClientMessage::decode(r#"[1,2]"#),
            Err(DecodeError::Malformed(_))
        ));
        assert_eq!(
            ClientMessage::decode(r#"{"t":"future.thing","id":"x1","extra":true}"#).unwrap(),
            ClientMessage::Unknown {
                t: "future.thing".into(),
                id: Some("x1".into())
            }
        );
        assert!(matches!(
            ClientMessage::decode(r#"{"t":"surface.subscribe","id":"s1","sub":0,"target":"p_1"}"#),
            Err(DecodeError::BadRequest { id: Some(id), .. }) if id == "s1"
        ));
        assert!(matches!(
            ClientMessage::decode(r#"{"t":"input.text","id":"i1","text":"y"}"#),
            Err(DecodeError::BadRequest { .. })
        ));
        assert!(matches!(
            ClientMessage::decode(
                r#"{"t":"input.text","id":"i1","sub":1,"target":"p","text":"y"}"#
            ),
            Err(DecodeError::BadRequest { .. })
        ));
        assert!(matches!(
            ClientMessage::decode(&format!(r#"{{"t":"ping","id":"{}"}}"#, "x".repeat(65))),
            Err(DecodeError::BadRequest { .. })
        ));
        let ClientMessage::SurfaceSubscribe(subscribe) = ClientMessage::decode(
            r#"{"t":"surface.subscribe","sub":3,"target":"p_1","mode":"sideways","max_fps":500,"new_field":1}"#,
        )
        .unwrap() else {
            panic!("not subscribe");
        };
        assert_eq!(subscribe.mode, SurfaceMode::Unknown);
        assert_eq!(subscribe.max_fps(), 60);
    }

    #[test]
    fn expo_token_shape() {
        assert!(valid_expo_token(
            "ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]"
        ));
        assert!(valid_expo_token("ExpoPushToken[a-b_c]"));
        assert!(!valid_expo_token("ExponentPushToken[]"));
        assert!(!valid_expo_token("ExponentPushToken[a b]"));
        assert!(!valid_expo_token("token"));
    }

    #[test]
    fn error_messages_carry_codes() {
        let text = error(Some("r7"), ErrorCode::NotFound, "pane p_9 not found");
        let value: Value = serde_json::from_str(&text).unwrap();
        let expected: Value = serde_json::from_str(&fixture("server.error.json")).unwrap();
        assert_eq!(value, expected);
    }
}
