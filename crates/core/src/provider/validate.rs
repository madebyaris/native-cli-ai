//! Lightweight API key validation per provider.

use nca_common::config::{NcaConfig, ProviderCompatibility, ProviderKind};

/// Result of an API key validation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Valid,
    InvalidKey(String),
    NetworkError(String),
}

use std::time::{Duration, Instant};

use reqwest::StatusCode;

/// Round-trip timing + validation (for `nca doctor` preflight).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub validation: ValidationResult,
    /// Time from sending the request until the response headers/body were received.
    pub round_trip_ms: u64,
}

/// Best-effort: open a TLS connection and hit the provider with the same probe as
/// [`validate_api_key`], using the **default** provider from `config`. Ignores errors.
pub async fn prewarm_default_provider(config: &NcaConfig) {
    let provider = config.provider.default;
    let Some(key) = config.provider.resolve_api_key_for(provider) else {
        return;
    };
    let base_url = config.provider.base_url_for(provider).to_string();
    let compat = config.provider.compatibility_for(provider);
    let _ = preflight_provider(provider, &key, &base_url, compat).await;
}

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
    preflight_provider(provider, api_key, base_url, compatibility)
        .await
        .validation
}

/// Same probes as [`validate_api_key`], plus round-trip latency in milliseconds.
pub async fn preflight_provider(
    provider: ProviderKind,
    api_key: &str,
    base_url: &str,
    compatibility: Option<ProviderCompatibility>,
) -> PreflightReport {
    let start = Instant::now();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return PreflightReport {
                validation: ValidationResult::NetworkError(format!("failed to build client: {e}")),
                round_trip_ms: start.elapsed().as_millis() as u64,
            };
        }
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

    let round_trip_ms = start.elapsed().as_millis() as u64;

    let validation = match result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                ValidationResult::Valid
            } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                ValidationResult::InvalidKey("Invalid API key — please check and try again".into())
            } else if status == StatusCode::BAD_REQUEST {
                ValidationResult::Valid
            } else {
                ValidationResult::NetworkError(format!("unexpected status: {status}"))
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
    };

    PreflightReport {
        validation,
        round_trip_ms,
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
