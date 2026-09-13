use base64::Engine as _;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::str::Utf8Error;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const MOBILE_PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMPATIBLE_MOBILE_VERSION: u16 = 1;

const PAIRING_DOMAIN: &[u8] = b"zed-mobile/pair/v1";
const AUTHENTICATION_DOMAIN: &[u8] = b"zed-mobile/auth/v1";
const PAIRING_URL_PREFIX: &str = "zed-mobile://pair?";

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid pairing URL")]
    InvalidPairingUrl,
    #[error("invalid base64url data: {0}")]
    InvalidBase64(#[source] base64::DecodeError),
    #[error("invalid UTF-8 data: {0}")]
    InvalidUtf8(#[source] Utf8Error),
    #[error("protocol payload field is too large")]
    PayloadTooLarge,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingOffer {
    pub offer_id: Uuid,
    pub endpoint: String,
    pub protocol_version: u16,
    pub host_public_key: String,
    pub pairing_secret: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl PairingOffer {
    pub fn encode_url(&self) -> Result<String, ProtocolError> {
        let encoded_offer = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(self)?);
        Ok(format!("zed-mobile://pair?code={encoded_offer}"))
    }

    pub fn decode_url(url: &str) -> Result<Self, ProtocolError> {
        let query = url
            .strip_prefix(PAIRING_URL_PREFIX)
            .ok_or(ProtocolError::InvalidPairingUrl)?;
        let mut parameters = query.split('&');
        let parameter = parameters
            .next()
            .ok_or(ProtocolError::InvalidPairingUrl)?;
        if parameters.next().is_some() {
            return Err(ProtocolError::InvalidPairingUrl);
        }
        let (name, encoded_offer) = parameter
            .split_once('=')
            .ok_or(ProtocolError::InvalidPairingUrl)?;
        if name != "code" || encoded_offer.is_empty() {
            return Err(ProtocolError::InvalidPairingUrl);
        }

        let decoded_offer = decode_base64url(encoded_offer)?;
        let encoded_json = std::str::from_utf8(&decoded_offer)
            .map_err(ProtocolError::InvalidUtf8)?;
        Ok(serde_json::from_str(encoded_json)?)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    StatusRead,
    ThreadsRead,
    ThreadsControl,
    TerminalStream,
    TerminalInput,
    ChatSend,
    AttachmentsSend,
    WorktreesRead,
    WorktreesCreate,
    FilesRead,
    SourceControlWrite,
    QuickCommands,
    AccountsRead,
    AccountsSwitch,
    UsageRead,
    Notifications,
    BrowserMobileView,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub host_name: String,
    pub protocol_version: u16,
    pub minimum_compatible_mobile_version: u16,
    #[serde(deserialize_with = "deserialize_sorted_capabilities")]
    pub capabilities: Vec<Capability>,
}

impl Status {
    pub fn new(
        host_name: String,
        protocol_version: u16,
        minimum_compatible_mobile_version: u16,
        mut capabilities: Vec<Capability>,
    ) -> Self {
        capabilities.sort_unstable();
        Self {
            host_name,
            protocol_version,
            minimum_compatible_mobile_version,
            capabilities,
        }
    }

    pub fn sort_capabilities(&mut self) {
        self.capabilities.sort_unstable();
    }
}

impl Serialize for Status {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut capabilities = self.capabilities.clone();
        capabilities.sort_unstable();
        let mut status = serializer.serialize_struct("Status", 4)?;
        status.serialize_field("host_name", &self.host_name)?;
        status.serialize_field("protocol_version", &self.protocol_version)?;
        status.serialize_field(
            "minimum_compatible_mobile_version",
            &self.minimum_compatible_mobile_version,
        )?;
        status.serialize_field("capabilities", &capabilities)?;
        status.end()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    PairBegin { offer_id: Uuid, client_nonce: String },
    PairComplete {
        offer_id: Uuid,
        pairing_secret: String,
        client_public_key: String,
        client_proof: String,
        device_label: String,
    },
    Connect { grant_id: Uuid, client_nonce: String },
    Authenticate { grant_id: Uuid, token: String, client_proof: String },
    Request {
        request_id: Uuid,
        operation_id: Option<Uuid>,
        method: String,
        params: serde_json::Value,
    },
    Ping { nonce: String },
}

impl ClientFrame {
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        Ok(serde_json::from_str(json)?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    PairChallenge {
        offer_id: Uuid,
        client_nonce: String,
        server_nonce: String,
        host_signature: String,
    },
    PairComplete { grant_id: Uuid, token: String },
    ServerChallenge {
        grant_id: Uuid,
        client_nonce: String,
        server_nonce: String,
        host_signature: String,
    },
    Authenticated { grant_id: Uuid },
    Response {
        request_id: Uuid,
        operation_id: Option<Uuid>,
        result: serde_json::Value,
    },
    Event { event: String, payload: serde_json::Value },
    Pong { nonce: String },
    Error {
        request_id: Option<Uuid>,
        code: String,
        message: String,
    },
}

impl ServerFrame {
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        Ok(serde_json::from_str(json)?)
    }
}

pub fn pairing_payload(
    protocol_version: u16,
    offer_id: Uuid,
    client_nonce: &str,
    server_nonce: &str,
) -> Result<Vec<u8>, ProtocolError> {
    let client_nonce = decode_base64url(client_nonce)?;
    let server_nonce = decode_base64url(server_nonce)?;
    let mut payload = Vec::with_capacity(
        PAIRING_DOMAIN.len() + 2 + 16 + 4 + client_nonce.len() + 4 + server_nonce.len(),
    );
    payload.extend_from_slice(PAIRING_DOMAIN);
    payload.extend_from_slice(&protocol_version.to_be_bytes());
    payload.extend_from_slice(offer_id.as_bytes());
    append_length_prefixed(&mut payload, &client_nonce)?;
    append_length_prefixed(&mut payload, &server_nonce)?;
    Ok(payload)
}

pub fn authentication_payload(
    protocol_version: u16,
    grant_id: Uuid,
    client_nonce: &str,
    server_nonce: &str,
    token_digest: &[u8; 32],
) -> Result<Vec<u8>, ProtocolError> {
    let client_nonce = decode_base64url(client_nonce)?;
    let server_nonce = decode_base64url(server_nonce)?;
    let mut payload = Vec::with_capacity(
        AUTHENTICATION_DOMAIN.len()
            + 2
            + 16
            + 4
            + client_nonce.len()
            + 4
            + server_nonce.len()
            + 4
            + token_digest.len(),
    );
    payload.extend_from_slice(AUTHENTICATION_DOMAIN);
    payload.extend_from_slice(&protocol_version.to_be_bytes());
    payload.extend_from_slice(grant_id.as_bytes());
    append_length_prefixed(&mut payload, &client_nonce)?;
    append_length_prefixed(&mut payload, &server_nonce)?;
    append_length_prefixed(&mut payload, token_digest)?;
    Ok(payload)
}

fn deserialize_sorted_capabilities<'de, D>(
    deserializer: D,
) -> Result<Vec<Capability>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut capabilities = Vec::<Capability>::deserialize(deserializer)?;
    capabilities.sort_unstable();
    Ok(capabilities)
}

fn decode_base64url(value: &str) -> Result<Zeroizing<Vec<u8>>, ProtocolError> {
    base64::engine::general_purpose::URL_SAFE
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value))
        .map(Zeroizing::new)
        .map_err(ProtocolError::InvalidBase64)
}

fn append_length_prefixed(payload: &mut Vec<u8>, value: &[u8]) -> Result<(), ProtocolError> {
    let length = u32::try_from(value.len()).map_err(|_| ProtocolError::PayloadTooLarge)?;
    payload.extend_from_slice(&length.to_be_bytes());
    payload.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::{Duration, OffsetDateTime};

    fn test_pairing_offer() -> PairingOffer {
        PairingOffer {
            offer_id: Uuid::from_u128(0x018f0aa68d3b7ccf9fe0cd23fb40ad40),
            endpoint: "ws://100.88.4.2:6769".to_owned(),
            protocol_version: MOBILE_PROTOCOL_VERSION,
            host_public_key: "base64url-key".to_owned(),
            pairing_secret: "base64url-secret".to_owned(),
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(1_789_214_700),
        }
    }

    #[test]
    fn pairing_offer_round_trips_through_json_and_url() -> Result<(), ProtocolError> {
        let offer = test_pairing_offer();
        let encoded_json = serde_json::to_string(&offer)?;
        assert_eq!(
            encoded_json,
            r#"{"offer_id":"018f0aa6-8d3b-7ccf-9fe0-cd23fb40ad40","endpoint":"ws://100.88.4.2:6769","protocol_version":1,"host_public_key":"base64url-key","pairing_secret":"base64url-secret","expires_at":"2026-09-12T12:05:00Z"}"#
        );

        let encoded_url = offer.encode_url()?;
        assert_eq!(
            PairingOffer::decode_url(&encoded_url)?.endpoint,
            "ws://100.88.4.2:6769"
        );
        assert_eq!(PairingOffer::decode_url(&encoded_url)?, offer);
        let padded_code = base64::engine::general_purpose::URL_SAFE.encode(encoded_json);
        assert_eq!(
            PairingOffer::decode_url(&format!("zed-mobile://pair?code={padded_code}"))?,
            offer
        );
        assert!(PairingOffer::decode_url("orca://pair?code=abc").is_err());
        assert!(PairingOffer::decode_url("zed-mobile://pair?code=not-json").is_err());
        assert!(PairingOffer::decode_url("zed-mobile://pair?code=abc&extra=def").is_err());
        assert!(PairingOffer::decode_url("zed-mobile://other?code=abc").is_err());
        assert!(PairingOffer::decode_url("zed-mobile://pair?code=abc&code=def").is_err());
        assert!(PairingOffer::decode_url("zed-mobile://pair?value=abc").is_err());
        Ok(())
    }

    #[test]
    fn client_request_and_authenticated_status_response_are_stable() -> Result<(), ProtocolError> {
        let request_id = Uuid::from_u128(1);
        let operation_id = Uuid::from_u128(2);
        let request = ClientFrame::Request {
            request_id,
            operation_id: Some(operation_id),
            method: "status.get".to_owned(),
            params: json!({}),
        };
        let encoded_request = request.to_json()?;
        assert_eq!(
            encoded_request,
            r#"{"type":"request","request_id":"00000000-0000-0000-0000-000000000001","operation_id":"00000000-0000-0000-0000-000000000002","method":"status.get","params":{}}"#
        );
        assert_eq!(ClientFrame::from_json(&encoded_request)?, request);
        assert!(ClientFrame::from_json(
            r#"{"type":"request","request_id":"00000000-0000-0000-0000-000000000000","operation_id":null,"method":"status.get","params":{}}"#
        )
        .is_ok());
        assert!(ClientFrame::from_json(
            r#"{"type":"request","method":"status.get","params":{}}"#
        )
        .is_err());

        let status = Status {
            host_name: "Zed Desktop".to_owned(),
            protocol_version: MOBILE_PROTOCOL_VERSION,
            minimum_compatible_mobile_version: MIN_COMPATIBLE_MOBILE_VERSION,
            capabilities: vec![Capability::ThreadsRead, Capability::StatusRead],
        };
        let response = ServerFrame::Response {
            request_id,
            operation_id: Some(operation_id),
            result: serde_json::to_value(&status)?,
        };
        assert_eq!(
            response.to_json()?,
            r#"{"type":"response","request_id":"00000000-0000-0000-0000-000000000001","operation_id":"00000000-0000-0000-0000-000000000002","result":{"host_name":"Zed Desktop","protocol_version":1,"minimum_compatible_mobile_version":1,"capabilities":["status_read","threads_read"]}}"#
        );
        assert_eq!(ServerFrame::from_json(&response.to_json()?)?, response);
        assert_eq!(
            serde_json::from_value::<Status>(serde_json::to_value(status)?)?.capabilities,
            vec![Capability::StatusRead, Capability::ThreadsRead]
        );
        let authenticated = ServerFrame::Authenticated { grant_id: Uuid::from_u128(3) };
        assert_eq!(
            serde_json::to_string(&authenticated)?,
            r#"{"type":"authenticated","grant_id":"00000000-0000-0000-0000-000000000003"}"#
        );
        Ok(())
    }

    #[test]
    fn capability_names_are_stable() -> Result<(), ProtocolError> {
        assert_eq!(serde_json::to_string(&Capability::BrowserMobileView)?, "\"browser_mobile_view\"");
        assert_eq!(serde_json::to_string(&Capability::SourceControlWrite)?, "\"source_control_write\"");
        Ok(())
    }

    #[test]
    fn all_envelopes_reject_unknown_fields_and_missing_required_data() {
        let invalid_frames = [
            r#"{"type":"pair_begin","offer_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","unexpected":true}"#,
            r#"{"type":"pair_begin","client_nonce":"nonce"}"#,
            r#"{"type":"pair_complete","offer_id":"00000000-0000-0000-0000-000000000001","pairing_secret":"secret","client_public_key":"key","client_proof":"proof"}"#,
            r#"{"type":"connect","grant_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","unexpected":true}"#,
            r#"{"type":"connect","client_nonce":"nonce"}"#,
            r#"{"type":"authenticate","grant_id":"00000000-0000-0000-0000-000000000001","token":"token","client_proof":"proof","unexpected":true}"#,
            r#"{"type":"authenticate","grant_id":"00000000-0000-0000-0000-000000000001","client_proof":"proof"}"#,
            r#"{"type":"request","request_id":"00000000-0000-0000-0000-000000000001","operation_id":null,"method":"status.get","params":{},"unexpected":true}"#,
            r#"{"type":"request","operation_id":null,"method":"status.get","params":{}}"#,
            r#"{"type":"ping","nonce":"nonce","unexpected":true}"#,
            r#"{"type":"ping"}"#,
        ];
        assert!(invalid_frames.iter().all(|frame| ClientFrame::from_json(frame).is_err()));

        assert!(serde_json::from_str::<Status>(
            r#"{"host_name":"Zed","protocol_version":1,"minimum_compatible_mobile_version":1,"capabilities":[],"unexpected":true}"#
        )
        .is_err());

        let invalid_server_frames = [
            r#"{"type":"pair_challenge","offer_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","server_nonce":"nonce","host_signature":"signature","unexpected":true}"#,
            r#"{"type":"pair_challenge","offer_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","server_nonce":"nonce"}"#,
            r#"{"type":"pair_complete","grant_id":"00000000-0000-0000-0000-000000000001","token":"token","unexpected":true}"#,
            r#"{"type":"pair_complete","grant_id":"00000000-0000-0000-0000-000000000001}"#,
            r#"{"type":"server_challenge","grant_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","server_nonce":"nonce","host_signature":"signature","unexpected":true}"#,
            r#"{"type":"server_challenge","grant_id":"00000000-0000-0000-0000-000000000001","client_nonce":"nonce","host_signature":"signature"}"#,
            r#"{"type":"authenticated","unexpected":true}"#,
            r#"{"type":"authenticated"}"#,
            r#"{"type":"response","request_id":"00000000-0000-0000-0000-000000000001","operation_id":null,"result":{},"unexpected":true}"#,
            r#"{"type":"response","request_id":"00000000-0000-0000-0000-000000000001","operation_id":null}"#,
            r#"{"type":"event","event":"status","payload":{},"unexpected":true}"#,
            r#"{"type":"event","event":"status"}"#,
            r#"{"type":"pong","nonce":"nonce","unexpected":true}"#,
            r#"{"type":"pong"}"#,
            r#"{"type":"error","request_id":null,"code":"error","message":"message","unexpected":true}"#,
            r#"{"type":"error","request_id":null,"message":"message"}"#,
        ];
        assert!(invalid_server_frames
            .iter()
            .all(|frame| ServerFrame::from_json(frame).is_err()));
    }

    #[test]
    fn signing_payload_vectors_are_byte_stable() -> Result<(), ProtocolError> {
        let offer_id = Uuid::from_u128(0x018f0aa68d3b7ccf9fe0cd23fb40ad40);
        assert_eq!(
            to_hex(&pairing_payload(
                MOBILE_PROTOCOL_VERSION,
                offer_id,
                "Y2xpZW50LW5vbmNl",
                "c2VydmVyLW5vbmNl",
            )?),
            "7a65642d6d6f62696c652f706169722f76310001018f0aa68d3b7ccf9fe0cd23fb40ad400000000c636c69656e742d6e6f6e63650000000c7365727665722d6e6f6e6365"
        );
        let grant_id = Uuid::from_u128(0x7f6f3d5d2f5b4c8fa1d8123456789abc);
        let token_digest = [
            0x90, 0x65, 0x66, 0xf5, 0x95, 0xb9, 0x0e, 0x05, 0x45, 0x7a, 0x4e, 0x12, 0xb0, 0xe9,
            0x2d, 0x81, 0x18, 0x4f, 0xee, 0xcc, 0xe6, 0x4b, 0x64, 0x23, 0x23, 0x8a, 0x4e, 0xeb,
            0x25, 0xeb, 0xf5, 0x67,
        ];
        assert_eq!(
            to_hex(&authentication_payload(
                MOBILE_PROTOCOL_VERSION,
                grant_id,
                "Y2xpZW50LW5vbmNl",
                "c2VydmVyLW5vbmNl",
                &token_digest,
            )?),
            "7a65642d6d6f62696c652f617574682f763100017f6f3d5d2f5b4c8fa1d8123456789abc0000000c636c69656e742d6e6f6e63650000000c7365727665722d6e6f6e636500000020906566f595b90e05457a4e12b0e92d81184feecce64b6423238a4eeb25ebf567"
        );
        Ok(())
    }

    #[test]
    fn invalid_nonce_encoding_is_rejected() {
        let offer_id = Uuid::from_u128(1);
        assert!(pairing_payload(MOBILE_PROTOCOL_VERSION, offer_id, "not valid", "YQ").is_err());
        assert!(authentication_payload(
            MOBILE_PROTOCOL_VERSION,
            offer_id,
            "YQ",
            "not valid",
            &[0; 32],
        )
        .is_err());
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
