use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::control::{ControlRequest, ControlResponse};
use crate::events::EventFrame;
use crate::pairing::{
    BindDeviceRequest, BindDeviceResponse, DeviceAuth, PairingConfirm, PairingInitiate,
    PairingRegister, PairingResume, PeerSessionReset, SubscriptionStatus,
};

/// Wire protocol version. Bumped only on incompatible envelope/message
/// changes. Adding new message types is forward-compatible (unknown
/// discriminators map to [`Message::Unknown`]) and does not require a bump.
pub const PROTO_VERSION: u32 = 1;

/// The raw wire frame exchanged between PC, relay, and phone.
///
/// `message_type` (serialized as `type`) is a free-form string so that
/// unknown discriminators always deserialize successfully. Typed
/// interpretation is done via [`Envelope::into_message`] /
/// [`Envelope::from_message`], which maps unknown types to
/// [`Message::Unknown`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub proto_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(default = "default_payload")]
    pub payload: Value,
}

fn default_payload() -> Value {
    Value::Null
}

/// Typed view of an [`Envelope`] payload, reached via
/// [`Envelope::into_message`]. Serialized adjacently
/// (`{"type":"..","payload":{..}}`) on the typed path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Message {
    ControlRequest(ControlRequest),
    ControlResponse(ControlResponse),
    Event(EventFrame),
    Heartbeat,
    PairingInitiate(PairingInitiate),
    PairingConfirm(PairingConfirm),
    PairingResume(PairingResume),
    PairingRegister(PairingRegister),
    PeerSessionReset(PeerSessionReset),
    DeviceAuth(DeviceAuth),
    BindDeviceRequest(BindDeviceRequest),
    BindDeviceResponse(BindDeviceResponse),
    SubscriptionStatus(SubscriptionStatus),
    /// Catch-all for unrecognized wire discriminators; carries the raw
    /// payload so newer peers' messages are not dropped by older peers.
    Unknown(Value),
}

/// Outer relay-routing shape that wraps a serialized [`Envelope`]. The
/// relay routes by `to_device_id` only and never inspects `ciphertext`;
/// encrypt/decrypt is owned by `relay-client`, not this crate.
///
/// The ciphertext is carried in exactly one of two encodings: the compact
/// `ciphertext_b64` (base64url-no-pad, ~1.33 chars per byte) emitted to peers
/// that advertised [`crate::pairing::CAPABILITY_CIPHERTEXT_B64`], or the
/// legacy `ciphertext` number array (~4 chars per byte) for old peers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedEnvelope {
    pub to_device_id: String,
    pub nonce: Vec<u8>,
    /// Legacy encoding: ciphertext bytes as a JSON number array. Emitted only
    /// by peers that predate `ciphertext_b64`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext: Option<Vec<u8>>,
    /// Compact encoding: ciphertext bytes as base64url-no-pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext_b64: Option<String>,
    /// Present when this frame is one fragment of a larger encrypted payload.
    /// All fragments share the same `chunk_id`; receivers reassemble them by
    /// `chunk_index` before decrypting the concatenated ciphertext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_total: Option<u32>,
    /// Payload encoding applied BEFORE encryption (e.g. `"gzip"`). Absent for
    /// small payloads sent as raw serialized JSON. Chunks inherit the value
    /// from the original envelope so reassembly can restore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

impl EncryptedEnvelope {
    /// Build from raw ciphertext bytes, choosing the compact base64 encoding
    /// when the receiver advertised [`crate::pairing::CAPABILITY_CIPHERTEXT_B64`]
    /// (`emit_b64`), else the legacy number array.
    pub fn from_ciphertext(
        to_device_id: String,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
        emit_b64: bool,
        encoding: Option<String>,
    ) -> Self {
        if emit_b64 {
            Self {
                to_device_id,
                nonce,
                ciphertext: None,
                ciphertext_b64: Some(ciphertext_b64_encode(&ciphertext)),
                chunk_id: None,
                chunk_index: None,
                chunk_total: None,
                encoding,
            }
        } else {
            Self {
                to_device_id,
                nonce,
                ciphertext: Some(ciphertext),
                ciphertext_b64: None,
                chunk_id: None,
                chunk_index: None,
                chunk_total: None,
                encoding,
            }
        }
    }

    /// Resolve the raw ciphertext bytes from whichever encoding is present.
    /// Prefers the compact `ciphertext_b64`; falls back to the legacy number
    /// array. Errors only when neither (or both invalid) is carried.
    pub fn ciphertext_bytes(&self) -> Result<Vec<u8>, String> {
        if let Some(b64) = &self.ciphertext_b64 {
            return decode_ciphertext_b64(b64);
        }
        if let Some(bytes) = &self.ciphertext {
            return Ok(bytes.clone());
        }
        Err("encrypted envelope carries no ciphertext".to_string())
    }
}

fn ciphertext_b64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_ciphertext_b64(encoded: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("invalid ciphertext_b64: {e}"))
}

impl Envelope {
    /// Build an envelope from a typed message, assigning the given
    /// request/response `id` (None for unsolicited events).
    pub fn from_message(id: Option<Uuid>, message: &Message) -> serde_json::Result<Self> {
        let value = serde_json::to_value(message)?;
        let (message_type, payload) = split_typed(value);
        Ok(Self {
            proto_version: PROTO_VERSION,
            id,
            message_type,
            payload,
        })
    }

    /// Interpret this envelope as a typed message. Unknown discriminators
    /// map to [`Message::Unknown`] carrying the raw payload.
    pub fn into_message(&self) -> serde_json::Result<Message> {
        Ok(match self.message_type.as_str() {
            "control_request" => {
                Message::ControlRequest(serde_json::from_value(self.payload.clone())?)
            }
            "control_response" => {
                Message::ControlResponse(serde_json::from_value(self.payload.clone())?)
            }
            "event" => Message::Event(serde_json::from_value(self.payload.clone())?),
            "heartbeat" => Message::Heartbeat,
            "pairing_initiate" => {
                Message::PairingInitiate(serde_json::from_value(self.payload.clone())?)
            }
            "pairing_confirm" => {
                Message::PairingConfirm(serde_json::from_value(self.payload.clone())?)
            }
            "pairing_resume" => {
                Message::PairingResume(serde_json::from_value(self.payload.clone())?)
            }
            "pairing_register" => {
                Message::PairingRegister(serde_json::from_value(self.payload.clone())?)
            }
            "peer_session_reset" => {
                Message::PeerSessionReset(serde_json::from_value(self.payload.clone())?)
            }
            "device_auth" => Message::DeviceAuth(serde_json::from_value(self.payload.clone())?),
            "bind_device_request" => {
                Message::BindDeviceRequest(serde_json::from_value(self.payload.clone())?)
            }
            "bind_device_response" => {
                Message::BindDeviceResponse(serde_json::from_value(self.payload.clone())?)
            }
            "subscription_status" => {
                Message::SubscriptionStatus(serde_json::from_value(self.payload.clone())?)
            }
            other => {
                let _ = other;
                Message::Unknown(self.payload.clone())
            }
        })
    }
}

/// Split an adjacently-tagged `Message` Value into its `(type, payload)`.
fn split_typed(value: Value) -> (String, Value) {
    match value {
        Value::Object(ref map) => {
            let message_type = map
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let payload = map.get("payload").cloned().unwrap_or(Value::Null);
            (message_type, payload)
        }
        other => ("unknown".to_string(), other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlRequest;
    use uuid::Uuid;
    use workspace_model::SessionStatus;

    #[test]
    fn envelope_roundtrip_preserves_all_fields() {
        let id = Uuid::new_v4();
        let env = Envelope {
            proto_version: PROTO_VERSION,
            id: Some(id),
            message_type: "control_request".to_string(),
            payload: serde_json::json!({"op":"cancel","request_id": id.to_string()}),
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn unknown_type_lands_in_unknown_variant() {
        let env = Envelope {
            proto_version: PROTO_VERSION,
            id: None,
            message_type: "some_future_message".to_string(),
            payload: serde_json::json!({"anything": 42}),
        };
        let msg = env.into_message().unwrap();
        match msg {
            Message::Unknown(v) => assert_eq!(v, serde_json::json!({"anything": 42})),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn control_request_roundtrips_and_echoes_request_id() {
        let request_id = Uuid::new_v4();
        let req = ControlRequest::Cancel { request_id };
        let env =
            Envelope::from_message(Some(request_id), &Message::ControlRequest(req.clone()))
                .unwrap();
        assert_eq!(env.id, Some(request_id));
        assert_eq!(env.message_type, "control_request");
        let msg = env.into_message().unwrap();
        match msg {
            Message::ControlRequest(ControlRequest::Cancel { request_id: rid }) => {
                assert_eq!(rid, request_id);
            }
            other => panic!("expected ControlRequest::Cancel, got {other:?}"),
        }
    }

    #[test]
    fn event_frame_roundtrips_through_envelope() {
        let env = Envelope::from_message(
            None,
            &Message::Event(EventFrame::SessionStatusChanged {
                session_id: "s-1".to_string(),
                status: SessionStatus::Idle,
            }),
        )
        .unwrap();
        assert_eq!(env.message_type, "event");
        match env.into_message().unwrap() {
            Message::Event(EventFrame::SessionStatusChanged { session_id, status }) => {
                assert_eq!(session_id, "s-1");
                assert_eq!(status, SessionStatus::Idle);
            }
            other => panic!("expected SessionStatusChanged, got {other:?}"),
        }
    }

    #[test]
    fn encrypted_envelope_exposes_no_plaintext() {
        let enc = EncryptedEnvelope::from_ciphertext(
            "dev-1".to_string(),
            vec![1, 2, 3],
            vec![4, 5, 6],
            false,
            None,
        );
        let json = serde_json::to_value(&enc).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert!(obj.contains_key("to_device_id"));
        assert!(obj.contains_key("nonce"));
        assert!(obj.contains_key("ciphertext"));
        for key in ["payload", "type", "id", "message_type", "proto_version", "encoding", "ciphertext_b64"] {
            assert!(!obj.contains_key(key), "unexpected key {key}");
        }
    }

    #[test]
    fn encrypted_envelope_emits_and_resolves_base64_ciphertext() {
        let bytes: Vec<u8> = (0..=255).cycle().take(512).collect();
        let enc = EncryptedEnvelope::from_ciphertext(
            "dev-1".to_string(),
            vec![0; 12],
            bytes.clone(),
            true,
            None,
        );
        assert!(enc.ciphertext.is_none());
        assert!(enc.ciphertext_b64.is_some());
        // The compact encoding must serialize smaller than the number array.
        let b64_len = serde_json::to_string(&enc).unwrap().len();
        let legacy = EncryptedEnvelope::from_ciphertext(
            "dev-1".to_string(),
            vec![0; 12],
            bytes.clone(),
            false,
            None,
        );
        let legacy_len = serde_json::to_string(&legacy).unwrap().len();
        assert!(
            b64_len * 2 < legacy_len,
            "base64 frame ({b64_len}) should be far smaller than number-array frame ({legacy_len})"
        );
        // Both encodings resolve to the same bytes; the legacy form also
        // decodes when it arrives as a string (cross-version tolerance).
        assert_eq!(enc.ciphertext_bytes().unwrap(), bytes);
        assert_eq!(legacy.ciphertext_bytes().unwrap(), bytes);
        let json = serde_json::to_string(&enc).unwrap();
        let back: EncryptedEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ciphertext_bytes().unwrap(), bytes);
    }

    #[test]
    fn encrypted_envelope_resolves_legacy_number_array() {
        // Old-peer frame: `ciphertext` as a number array, no b64 field.
        let json = r#"{"to_device_id":"dev-1","nonce":[1],"ciphertext":[9,8,7]}"#;
        let enc: EncryptedEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(enc.ciphertext_bytes().unwrap(), vec![9, 8, 7]);
    }

    #[test]
    fn peer_session_reset_roundtrips_and_omits_empty_payload() {
        let env =
            Envelope::from_message(None, &Message::PeerSessionReset(PeerSessionReset {})).unwrap();
        assert_eq!(env.message_type, "peer_session_reset");
        let json = serde_json::to_string(&env).unwrap();
        let back = serde_json::from_str::<Envelope>(&json).unwrap();
        match back.into_message().unwrap() {
            Message::PeerSessionReset(_) => {}
            other => panic!("expected PeerSessionReset, got {other:?}"),
        }
    }
}
