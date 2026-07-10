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
            if !redaction_separator_token(token) {
                redact_following = redact_following.saturating_sub(1);
                extend_secret_redaction(&lower, &mut redact_following, &mut redact_rest);
            }
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
    if has_secret_key(lower, "private_key") || lower.contains("-----begin") {
        *rest = true;
    } else if has_secret_key(lower, "authorization") {
        if authorization_needs_following_value(lower) {
            *following = (*following).max(1);
        }
    } else if ["access_token", "refresh_token", "client_secret"]
        .into_iter()
        .find(|key| has_secret_key(lower, key))
        .is_some_and(|key| assigned_value_after_key(lower, key).is_none_or(str::is_empty))
        || scheme_needs_following_value(lower)
    {
        *following = (*following).max(1);
    }
}

fn has_secret_key(lower: &str, key: &str) -> bool {
    lower
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|segment| segment == key)
}

fn assigned_value_after_key<'a>(lower: &'a str, key: &str) -> Option<&'a str> {
    let start = lower.find(key)? + key.len();
    let tail = &lower[start..];
    let separator = tail.find([':', '='])?;
    Some(
        tail[separator + 1..]
            .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))),
    )
}

fn authorization_needs_following_value(lower: &str) -> bool {
    match assigned_value_after_key(lower, "authorization") {
        None | Some("") | Some("bearer" | "basic") => true,
        Some(_) => false,
    }
}

fn scheme_needs_following_value(lower: &str) -> bool {
    matches!(
        lower.trim_matches(|ch: char| !ch.is_ascii_alphanumeric()),
        "bearer" | "basic"
    )
}

fn redaction_separator_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
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
    [
        "access_token",
        "refresh_token",
        "client_secret",
        "private_key",
        "authorization",
    ]
    .into_iter()
    .any(|key| has_secret_key(&lower, key))
        || scheme_needs_following_value(&lower)
        || lower.contains("-----begin")
        || lower
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
            .starts_with("ya29.")
}

#[cfg(test)]
mod tests {
    use super::redact_secret_text;

    #[test]
    fn redacts_secret_bearing_tokens() {
        for (source, expected) in [
            (
                "authorization: Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted] ok",
            ),
            (
                "Authorization : Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted] ok",
            ),
            (
                "Authorization=Bearer opaque-secret ok",
                "[redacted] [redacted] ok",
            ),
            (
                "access_token = opaque-secret ok",
                "[redacted] [redacted] [redacted] ok",
            ),
            (
                "\"access_token\" : \"opaque-secret\" ok",
                "[redacted] [redacted] [redacted] ok",
            ),
            (
                "\"Authorization\" : \"Bearer opaque-secret\" ok",
                "[redacted] [redacted] [redacted] [redacted] ok",
            ),
            (
                "client_secret : opaque-secret ok",
                "[redacted] [redacted] [redacted] ok",
            ),
            ("access_token=opaque-secret ok", "[redacted] ok"),
            ("ya29.synthetic ok", "[redacted] ok"),
            ("safe authorization_failed message", "safe authorization_failed message"),
        ] {
            assert_eq!(redact_secret_text(source), expected);
        }

        let private_key = redact_secret_text("private_key: -----BEGIN PRIVATE KEY----- secret");
        assert!(!private_key.contains("BEGIN"));
        assert!(!private_key.contains("secret"));
    }
}
