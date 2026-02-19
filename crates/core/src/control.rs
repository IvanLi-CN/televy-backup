use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub fn control_ipc_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ipc").join("control.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default)]
    pub details: serde_json::Value,
}

impl ControlError {
    pub fn unavailable(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            code: "control.unavailable".to_string(),
            message: message.into(),
            retryable: true,
            details,
        }
    }

    pub fn timeout(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            code: "control.timeout".to_string(),
            message: message.into(),
            retryable: true,
            details,
        }
    }

    pub fn invalid_request(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            code: "control.invalid_request".to_string(),
            message: message.into(),
            retryable: false,
            details,
        }
    }

    pub fn method_not_found(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            code: "control.method_not_found".to_string(),
            message: message.into(),
            retryable: false,
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    #[serde(rename = "type")]
    pub type_: String,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl ControlRequest {
    pub fn new(
        id: impl Into<String>,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Self {
        Self {
            type_: "control.request".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    #[serde(rename = "type")]
    pub type_: String,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl ControlResponse {
    pub fn ok(id: impl Into<String>, result: serde_json::Value) -> Self {
        Self {
            type_: "control.response".to_string(),
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: impl Into<String>, error: ControlError) -> Self {
        Self {
            type_: "control.response".to_string(),
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatusResult {
    pub backend: String,
    pub key_present: bool,
    pub keychain_disabled: bool,
    pub vault_key_file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsPresenceParams {
    pub endpoint_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsPresenceResult {
    pub master_key_present: bool,
    pub telegram_mtproto_api_hash_present: bool,
    pub telegram_bot_token_present_by_endpoint: serde_json::Map<String, serde_json::Value>,
    pub telegram_mtproto_session_present_by_endpoint: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSetTelegramBotTokenParams {
    pub endpoint_id: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSetTelegramApiHashParams {
    pub api_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsClearTelegramMtprotoSessionParams {
    pub endpoint_id: String,
}

// Best-effort status reporting from CLI -> daemon for UI status surfaces.
// These calls must not be required for correctness; they only improve observability.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTaskStartParams {
    pub task_id: String,
    pub kind: String, // "backup" | "restore" | "verify"
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTaskProgress {
    pub phase: String,
    pub files_total: Option<u64>,
    pub files_done: Option<u64>,
    pub chunks_total: Option<u64>,
    pub chunks_done: Option<u64>,
    pub bytes_read: Option<u64>,
    pub bytes_uploaded: Option<u64>,
    pub bytes_downloaded: Option<u64>,
    pub bytes_deduped: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTaskProgressParams {
    pub task_id: String,
    pub kind: String, // "backup" | "restore" | "verify"
    pub target_id: String,
    pub progress: StatusTaskProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTaskFinishParams {
    pub task_id: String,
    pub kind: String, // "backup" | "restore" | "verify"
    pub target_id: String,
    pub state: String, // "succeeded" | "failed"
}
