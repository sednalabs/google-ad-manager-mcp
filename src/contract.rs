//! Shared Contract V1 response helpers.

use std::time::Instant;

use mcp_toolkit::rmcp::model::CallToolResult;
use mcp_toolkit_scratchpad::ScratchpadError;
use serde_json::{Map, Value, json};

use crate::AdManagerError;

pub fn success(data: Value, started: Instant) -> CallToolResult {
    CallToolResult::structured(json!({
        "ok": true,
        "data": data,
        "meta": {
            "elapsed_ms": elapsed_ms(started),
        }
    }))
}

pub fn success_with_meta(data: Value, meta: Value, started: Instant) -> CallToolResult {
    CallToolResult::structured(success_envelope_with_meta(data, meta, started))
}

pub(crate) fn success_envelope_with_meta(data: Value, meta: Value, started: Instant) -> Value {
    let mut meta_map = match meta {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    meta_map.insert("elapsed_ms".to_string(), json!(elapsed_ms(started)));
    json!({
        "ok": true,
        "data": data,
        "meta": meta_map,
    })
}

pub fn error(err: AdManagerError, started: Instant) -> CallToolResult {
    CallToolResult::structured(json!({
        "ok": false,
        "error": {
            "code": err.code(),
            "reason": err.reason(),
            "message": redact_secret_text(&err.to_string()),
            "category": err.category(),
            "hint": err.hint(),
        },
        "meta": {
            "elapsed_ms": elapsed_ms(started),
        }
    }))
}

pub(crate) fn result_contract_error(
    field: &'static str,
    message: impl AsRef<str>,
    started: Instant,
) -> CallToolResult {
    CallToolResult::structured(json!({
        "ok": false,
        "error": {
            "code": "result_contract_error",
            "reason": "result_contract_failed",
            "message": redact_secret_text(&format!(
                "result contract failed for {field}: {}",
                message.as_ref()
            )),
            "category": "safety",
            "hint": "Narrow the target or page limits and omit optional raw output; report an adapter defect if a bounded projection still fails.",
        },
        "meta": {
            "elapsed_ms": elapsed_ms(started),
        }
    }))
}

pub fn error_with_detail(err: AdManagerError, detail: Value, started: Instant) -> CallToolResult {
    CallToolResult::structured(json!({
        "ok": false,
        "error": {
            "code": err.code(),
            "reason": err.reason(),
            "message": redact_secret_text(&err.to_string()),
            "category": err.category(),
            "hint": err.hint(),
            "detail": redact_secret_value(detail),
        },
        "meta": {
            "elapsed_ms": elapsed_ms(started),
        }
    }))
}

pub fn scratchpad_error(err: ScratchpadError, started: Instant) -> CallToolResult {
    CallToolResult::structured(json!({
        "ok": false,
        "error": {
            "code": err.code(),
            "reason": err.reason(),
            "message": redact_secret_text(&err.to_string()),
            "category": err.category(),
            "detail": err.detail(),
            "hint": err.hint(),
        },
        "meta": {
            "elapsed_ms": elapsed_ms(started),
        }
    }))
}

pub fn redact_secret_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut redact_following = 0_usize;
    let mut redact_rest = false;
    for token in input.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let lower = token.to_ascii_lowercase();
        if redact_rest {
            out.push_str("[redacted]");
        } else if redact_following > 0 {
            out.push_str("[redacted]");
            redact_following = redact_following.saturating_sub(1);
            extend_secret_redaction(&lower, &mut redact_following, &mut redact_rest);
        } else if looks_secret_bearing(token) {
            out.push_str("[redacted]");
            extend_secret_redaction(&lower, &mut redact_following, &mut redact_rest);
        } else {
            out.push_str(token);
        }
    }
    out
}

fn extend_secret_redaction(lower: &str, following: &mut usize, rest: &mut bool) {
    if lower.contains("private_key") || lower.contains("-----begin") {
        *rest = true;
    } else if lower.contains("authorization:") || lower == "authorization" {
        *following = (*following).max(3);
    } else if matches!(lower, "bearer" | "basic")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("client_secret")
    {
        *following = (*following).max(1);
    }
}

pub fn redact_secret_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_secret_text(&text)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(redact_secret_value)
                .collect::<Vec<_>>(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_secret_value(value)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    let elapsed = started.elapsed().as_millis();
    if elapsed > u128::from(u64::MAX) {
        u64::MAX
    } else {
        elapsed as u64
    }
}

fn looks_secret_bearing(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("client_secret")
        || lower.contains("private_key")
        || lower.contains("authorization:")
        || matches!(lower.as_str(), "authorization" | "bearer" | "basic")
        || lower.contains("-----begin")
        || lower.starts_with("ya29.")
}

#[cfg(test)]
mod tests {
    use super::redact_secret_text;

    #[test]
    fn redacts_secret_bearing_tokens() {
        let redacted = redact_secret_text(
            "authorization: Bearer opaque-secret access_token next-secret ok",
        );
        assert!(!redacted.contains("opaque-secret"));
        assert!(!redacted.contains("next-secret"));
        assert!(redacted.contains("ok"));

        let spaced = redact_secret_text("Authorization : Bearer another-opaque-secret ok");
        assert!(!spaced.contains("another-opaque-secret"));

        let private_key = redact_secret_text("private_key: -----BEGIN PRIVATE KEY----- secret");
        assert!(!private_key.contains("BEGIN"));
        assert!(!private_key.contains("secret"));
    }
}
