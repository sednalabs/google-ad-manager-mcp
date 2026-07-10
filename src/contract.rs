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
    let mut redact_rest = false;
    let tokens = input.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        let token = *token;
        let following = &tokens[index + 1..];
        if !out.is_empty() {
            out.push(' ');
        }
        let lower = token.to_ascii_lowercase();
        if redact_rest {
            out.push_str("[redacted]");
        } else if looks_secret_bearing(token, following) {
            out.push_str("[redacted]");
            redact_rest = secret_value_extends_past_token(&lower);
        } else {
            out.push_str(token);
        }
    }
    out
}

fn secret_value_extends_past_token(lower: &str) -> bool {
    if lower.contains("-----begin") {
        return true;
    }
    for key in [
        "private_key",
        "authorization",
        "access_token",
        "refresh_token",
        "client_secret",
    ] {
        if lower.match_indices(key).any(|(start, _)| {
            credential_occurrence_needs_following_value(lower, start, key)
        }) {
            return true;
        }
    }
    for scheme in ["bearer", "basic"] {
        if lower
            .match_indices(scheme)
            .any(|(start, _)| !marker_has_inline_material(lower, start, scheme.len()))
        {
            return true;
        }
    }
    lower
        .match_indices("ya29.")
        .any(|(start, _)| !marker_has_inline_material(lower, start, "ya29.".len()))
}

fn compound_secret_key_start(lower: &str, key: &str) -> Option<usize> {
    lower.match_indices(key).find_map(|(start, _)| {
        let before = lower[..start].chars().next_back();
        let tail = &lower[start + key.len()..];
        let after = tail.chars().next();
        let before_is_boundary = before.is_none_or(|ch| !ch.is_ascii_alphanumeric());
        if !before_is_boundary || after.is_some_and(|ch| ch.is_ascii_alphanumeric()) {
            return None;
        }
        Some(start)
    })
}

fn benign_secret_status_suffix(tail: &str) -> bool {
    matches!(
        tail,
        "_check"
            | "_check_failed"
            | "_configured"
            | "_disabled"
            | "_error"
            | "_expired"
            | "_failed"
            | "_failure"
            | "_missing"
            | "_present"
            | "_rotation"
            | "_rotation_failed"
            | "_status"
            | "_unavailable"
            | "_validation"
            | "_validation_failed"
    )
}

fn assigned_value_after_key<'a>(lower: &'a str, key: &str) -> Option<&'a str> {
    let start = compound_secret_key_start(lower, key)?;
    assigned_value_after_occurrence(lower, start, key)
}

fn assigned_value_after_occurrence<'a>(
    lower: &'a str,
    start: usize,
    key: &str,
) -> Option<&'a str> {
    let tail = &lower[start + key.len()..];
    let separator = tail.find([':', '='])?;
    if tail[..separator]
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
    {
        return None;
    }
    Some(tail[separator + 1..].trim_matches(|ch| !credential_value_char(ch)))
}

fn credential_occurrence_needs_following_value(lower: &str, start: usize, key: &str) -> bool {
    match assigned_value_after_occurrence(lower, start, key) {
        None | Some("") => true,
        Some("bearer" | "basic") if key == "authorization" => true,
        Some(_) => false,
    }
}

fn marker_has_inline_material(lower: &str, start: usize, marker_len: usize) -> bool {
    lower[..start]
        .chars()
        .chain(lower[start + marker_len..].chars())
        .any(char::is_alphanumeric)
}

fn credential_value_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn inline_value_after_key(lower: &str, key: &str) -> bool {
    let Some(start) = compound_secret_key_start(lower, key) else {
        return false;
    };
    !lower[start + key.len()..].is_empty()
}

fn benign_secret_status_phrase(lower: &str, key: &str, following: &[&str]) -> bool {
    let Some(start) = compound_secret_key_start(lower, key) else {
        return false;
    };
    let tail = &lower[start + key.len()..];
    if start != 0 || !benign_secret_status_suffix(tail) {
        return false;
    }
    if following.is_empty() {
        return true;
    }
    (tail == "_rotation_failed" && exact_ascii_phrase(following, &["please", "retry"]))
        || (tail == "_missing" && exact_ascii_phrase(following, &["use", "workload", "identity"]))
}

fn exact_ascii_phrase(actual: &[&str], expected: &[&str]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
}

fn authorization_needs_following_value(lower: &str) -> bool {
    match assigned_value_after_key(lower, "authorization") {
        None | Some("") | Some("bearer" | "basic") => true,
        Some(_) => false,
    }
}

fn contains_scheme_marker(lower: &str) -> bool {
    lower.contains("bearer") || lower.contains("basic")
}

fn redaction_separator_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
}

fn credential_key_starts_value(lower: &str, key: &str, following: &[&str]) -> bool {
    if !lower.contains(key) {
        return false;
    }
    if benign_secret_status_phrase(lower, key, following) {
        return false;
    }
    let Some(start) = compound_secret_key_start(lower, key) else {
        return true;
    };
    if start != 0 {
        return true;
    }
    assigned_value_after_key(lower, key).is_some()
        || inline_value_after_key(lower, key)
        || following.first().is_some_and(|value| {
            key != "authorization"
                || lower != "authorization"
                || redaction_separator_token(value)
                || !benign_authorization_diagnostic_phrase(following)
        })
}

fn scheme_starts_credential(lower: &str, following: &[&str]) -> bool {
    if !contains_scheme_marker(lower) {
        return false;
    }
    !matches!(lower, "bearer" | "basic") || following.first().is_some_and(|next| !next.is_empty())
}

fn benign_authorization_diagnostic_phrase(following: &[&str]) -> bool {
    let [qualifier] = following else {
        return false;
    };
    [
        "authentication",
        "authorization",
        "check",
        "configured",
        "disabled",
        "error",
        "expired",
        "failed",
        "failure",
        "missing",
        "mode",
        "present",
        "rotation",
        "status",
        "support",
        "unavailable",
        "validation",
    ]
    .into_iter()
    .any(|allowed| qualifier.eq_ignore_ascii_case(allowed))
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

fn looks_secret_bearing(token: &str, following: &[&str]) -> bool {
    let lower = token.to_ascii_lowercase();
    [
        "access_token",
        "refresh_token",
        "client_secret",
        "private_key",
        "authorization",
    ]
    .into_iter()
    .any(|key| credential_key_starts_value(&lower, key, following))
        || scheme_starts_credential(&lower, following)
        || lower.contains("-----begin")
        || lower.contains("ya29.")
}

#[cfg(test)]
mod tests {
    use super::redact_secret_text;

    #[test]
    fn redacts_secret_bearing_tokens() {
        for (source, expected) in [
            (
                "authorization: Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "Authorization : Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "Authorization=Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted]",
            ),
            (
                "access_token = opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "\"access_token\" : \"opaque-secret\" ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "\"Authorization\" : \"Bearer opaque-secret\" ok",
                "[redacted] [redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "client_secret : opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "http_authorization=Bearer opaque-secret ok",
                "[redacted] [redacted] [redacted]",
            ),
            (
                "proxy_authorization: opaque-secret ok",
                "[redacted] [redacted] [redacted]",
            ),
            ("proxy_authorization:opaque-secret ok", "[redacted] ok"),
            ("google_access_token=opaque-secret ok", "[redacted] ok"),
            ("oauth_client_secret=opaque-secret ok", "[redacted] ok"),
            (
                "service_account_private_key=opaque-secret ok",
                "[redacted] ok",
            ),
            ("access_token_value=opaque-secret ok", "[redacted] ok"),
            ("private_key_material=opaque-secret ok", "[redacted] ok"),
            (
                "access_token is opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "access_token has value opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "Authorization is opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "proxy_authorization has value opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted] [redacted]",
            ),
            (
                "Bearer credential opaque-secret ok",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            ("access_token=opaque-secret ok", "[redacted] ok"),
            ("ya29.synthetic ok", "[redacted] ok"),
            ("authorization failed", "authorization failed"),
            (
                "Authorization failed opaque-secret",
                "[redacted] [redacted] [redacted]",
            ),
            (
                "basic validation failed",
                "[redacted] [redacted] [redacted]",
            ),
            (
                "access_token missing opaque-secret",
                "[redacted] [redacted] [redacted]",
            ),
            (
                "private_key_rotation_failed please retry",
                "private_key_rotation_failed please retry",
            ),
            (
                "client_secret_missing use workload identity",
                "client_secret_missing use workload identity",
            ),
            ("client_secret_missing-opaque-secret", "[redacted]"),
            (
                "client_secret_missing opaque-secret",
                "[redacted] [redacted]",
            ),
            ("client_secret_missing\u{79d8}\u{5bc6}", "[redacted]"),
            (
                "client_secret_missing use workload identity\u{79d8}\u{5bc6}",
                "[redacted] [redacted] [redacted] [redacted]",
            ),
            ("access_token\u{79d8}\u{5bc6}", "[redacted]"),
            ("opaque-secret_client_secret_missing", "[redacted]"),
            ("opaqueclient_secret_missing", "[redacted]"),
            (
                "Authorization failed\u{79d8}\u{5bc6}",
                "[redacted] [redacted]",
            ),
            ("Authorization [failed]", "[redacted] [redacted]"),
            ("opaque_authorization failed", "[redacted] [redacted]"),
            (
                "opaque-secret_authorization failed",
                "[redacted] [redacted]",
            ),
            (
                "\u{79d8}\u{5bc6}authorization failed",
                "[redacted] [redacted]",
            ),
            ("Bearer\u{79d8}\u{5bc6}", "[redacted]"),
            ("opaqueBearer", "[redacted]"),
            ("opaqueya29.synthetic", "[redacted]"),
            ("Bearer", "Bearer"),
            ("opaque_access_token", "[redacted]"),
            ("\u{79d8}\u{5bc6}_access_token", "[redacted]"),
            (
                "access_token=masked;client_secret opaque-secret",
                "[redacted] [redacted]",
            ),
            ("ya29. opaque-secret", "[redacted] [redacted]"),
            ("opaqueya29.synthetic ok", "[redacted] ok"),
            ("Bearer=opaque ok", "[redacted] ok"),
            (
                "access_token=masked;client_secret=masked ok",
                "[redacted] ok",
            ),
            (
                "access_token;reason=missing opaque-secret",
                "[redacted] [redacted]",
            ),
        ] {
            assert_eq!(redact_secret_text(source), expected);
        }

        let private_key = redact_secret_text("private_key: -----BEGIN PRIVATE KEY----- secret");
        assert!(!private_key.contains("BEGIN"));
        assert!(!private_key.contains("secret"));
    }
}
