//! Lightweight API key validation per provider.

use nca_common::config::{ProviderCompatibility, ProviderKind};

/// Result of an API key validation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Valid,
    InvalidKey(String),
    NetworkError(String),
}

use std::time::Duration;

use reqwest::StatusCode;

/// Validate an API key by making a lightweight request to the provider.
///
/// - OpenAI / OpenRouter: `GET /v1/models`
/// - Anthropic / MiniMax: `POST /v1/messages` with minimal body
pub async fn validate_api_key(
    provider: ProviderKind,
    api_key: &str,
    base_url: &str,
    compatibility: Option<ProviderCompatibility>,
) -> ValidationResult {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ValidationResult::NetworkError(format!("failed to build client: {e}")),
    };

    let result = match provider {
        ProviderKind::OpenAi | ProviderKind::OpenRouter => {
            let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
            client
                .get(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .send()
                .await
        }
        ProviderKind::Anthropic | ProviderKind::MiniMax => {
            // Send a minimal POST with an intentionally empty body.
            // A valid key returns 400 (bad request); an invalid key returns 401/403.
            // This avoids coupling validation to any specific model ID.
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
            client
                .post(&url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .body(r#"{"max_tokens":1,"messages":[]}"#)
                .send()
                .await
        }
        ProviderKind::Custom => match compatibility.unwrap_or(ProviderCompatibility::OpenAi) {
            ProviderCompatibility::OpenAi => {
                let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
                client
                    .get(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .send()
                    .await
            }
            ProviderCompatibility::Anthropic => {
                let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
                client
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .body(r#"{"max_tokens":1,"messages":[]}"#)
                    .send()
                    .await
            }
        },
    };

    match result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                ValidationResult::Valid
            } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                ValidationResult::InvalidKey("Invalid API key — please check and try again".into())
            } else {
                // Some providers return 400 for minimal requests but the key is valid.
                // A 400 with auth headers accepted means the key works.
                if status == StatusCode::BAD_REQUEST {
                    ValidationResult::Valid
                } else {
                    ValidationResult::NetworkError(format!("unexpected status: {status}"))
                }
            }
        }
        Err(e) => {
            if e.is_timeout() {
                ValidationResult::NetworkError(
                    "Connection timed out — check your network and try again".into(),
                )
            } else {
                ValidationResult::NetworkError(format!(
                    "Connection failed — check your network and try again ({e})"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_result_variants_exist() {
        let valid = ValidationResult::Valid;
        let invalid = ValidationResult::InvalidKey("bad key".into());
        let net_err = ValidationResult::NetworkError("timeout".into());
        assert_eq!(valid, ValidationResult::Valid);
        assert!(matches!(invalid, ValidationResult::InvalidKey(_)));
        assert!(matches!(net_err, ValidationResult::NetworkError(_)));
    }

    #[test]
    fn invalid_key_message_preserved() {
        let msg = "Invalid API key — please check and try again";
        let result = ValidationResult::InvalidKey(msg.into());
        match result {
            ValidationResult::InvalidKey(m) => assert_eq!(m, msg),
            _ => panic!("expected InvalidKey"),
        }
    }

    #[test]
    fn network_error_message_preserved() {
        let msg = "Connection timed out — check your network and try again";
        let result = ValidationResult::NetworkError(msg.into());
        match result {
            ValidationResult::NetworkError(m) => assert_eq!(m, msg),
            _ => panic!("expected NetworkError"),
        }
    }
}
