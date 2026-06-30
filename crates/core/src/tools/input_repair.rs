//! Tool input repair layer for common LLM tool-calling mistakes.
//!
//! Implements the "validate-then-repair" pattern described at
//! <https://commandcode.ai/blog/how-did-we-make-deepseek-outperform-claude-opus-4.7>:
//!
//! 1. Try to deserialize normally — valid inputs are **never touched**.
//! 2. On failure, apply targeted repairs to the [`serde_json::Value`].
//! 3. Re-attempt deserialization; on success, log telemetry.
//! 4. On continued failure, return a model-readable error.
//!
//! The four shape repairs handle ~90% of tool-calling failures from
//! DeepSeek, Qwen, GLM, and similar models. An additional repair strips
//! markdown auto-link leakage from path fields.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Apply all repairs to a [`serde_json::Value`] and return the (possibly
/// mutated) clone. The original is never modified.
///
/// This function is **idempotent on valid inputs**: if no repairs are needed,
/// the returned value is a cheap clone.
pub fn repair_value(input: &Value) -> Value {
    let mut v = input.clone();
    strip_null_optional_fields(&mut v);
    parse_stringified_json_values(&mut v);
    wrap_bare_object_as_array(&mut v);
    wrap_bare_string_as_array(&mut v);
    sanitize_markdown_autolink_paths(&mut v);
    v
}

/// Try to repair a raw JSON string that failed `serde_json::from_str`.
/// Handles common truncation/encoding issues from streaming SSE.
pub fn repair_json_string(raw: &str) -> Option<Value> {
    // Attempt 1: parse as-is
    if let Ok(v) = serde_json::from_str(raw) {
        return Some(v);
    }
    // Attempt 2: trim whitespace
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Some(v);
    }
    // Attempt 3: strip trailing commas before } or ]
    let no_trailing = strip_trailing_commas(trimmed);
    if let Ok(v) = serde_json::from_str(&no_trailing) {
        return Some(v);
    }
    // Attempt 4: the stream may have truncated — find the last balanced
    // brace/bracket and try parsing up to that point.
    if no_trailing.len() <= 64 * 1024
        && let Some(idx) = find_last_balanced_brace(&no_trailing)
        && let Ok(v) = serde_json::from_str(&no_trailing[..=idx])
    {
        return Some(v);
    }
    None
}

/// Remove trailing commas before closing braces/brackets.
/// E.g. `{"a": 1,}` → `{"a": 1}`
fn strip_trailing_commas(s: &str) -> String {
    let mut result = s.to_string();
    // Walk backwards; when we see `}` or `]`, remove the preceding `,`
    // skipping over whitespace.
    let mut i = result.len();
    loop {
        if i == 0 {
            break;
        }
        i -= 1;
        let ch = result.as_bytes()[i];
        if ch == b'}' || ch == b']' {
            let mut j = i;
            while j > 0 {
                let prev = result.as_bytes()[j - 1];
                if prev == b' ' || prev == b'\n' || prev == b'\r' || prev == b'\t' {
                    j -= 1;
                } else {
                    break;
                }
            }
            if j > 0 && result.as_bytes()[j - 1] == b',' {
                result.remove(j - 1);
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Repair #1: Strip null for optional fields
// ---------------------------------------------------------------------------

/// Remove keys whose value is `Null` but that are **not** in the required set.
///
/// Since we don't have the schema here, we conservatively strip nulls only
/// from the top-level object. Serde's `#[serde(default)]` handles the rest
/// during deserialization — this repair ensures the null never reaches the
/// deserializer for fields that lack a default.
fn strip_null_optional_fields(v: &mut Value) {
    if let Value::Object(map) = v {
        map.retain(|_, v| !v.is_null());
    }
}

// ---------------------------------------------------------------------------
// Repair #2: Parse stringified JSON values
// ---------------------------------------------------------------------------

/// If a field's value is a JSON string that itself parses to a JSON value
/// (array or object), replace it with the parsed value.
///
/// Order matters: this **must** run before `wrap_bare_string_as_array`
/// or a stringified-array like `'["a","b"]'` would be wrapped to
/// `["[\"a\",\"b\"]"]` instead of being parsed to `["a", "b"]`.
fn parse_stringified_json_values(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (_, val) in map.iter_mut() {
                parse_stringified_json_values(val);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                parse_stringified_json_values(item);
            }
        }
        Value::String(s) => {
            let trimmed = s.trim();
            if (trimmed.starts_with('[') || trimmed.starts_with('{'))
                && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
            {
                // Only replace if the parsed value is an array or object
                // (not a bare string or number, which would be valid JSON too).
                if parsed.is_array() || parsed.is_object() {
                    *v = parsed;
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Repair #3: Wrap single object as array
// ---------------------------------------------------------------------------

/// If a field should be an array but the model sent a single object `{}`,
/// wrap it in an array.
///
/// We can only guess at the schema here, so we only apply this when the
/// field name suggests an array (contains "edits", "items", "matches", etc.)
/// or when the value is an object whose keys look like array-item keys
/// (e.g., `old_text`/`new_text` for edits).
fn wrap_bare_object_as_array(v: &mut Value) {
    if let Value::Object(map) = v {
        for (key, val) in map.iter_mut() {
            if let Value::Object(_) = val {
                // Heuristic: field names that commonly expect arrays
                if looks_like_array_field(key) {
                    let obj = std::mem::take(val);
                    *val = Value::Array(vec![obj]);
                }
            }
        }
    }
}

/// Heuristic: does this field name suggest it should be an array?
fn looks_like_array_field(name: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        "s",    // edits, items, matches, options
        "list", // file_list
        "entries",
        "files",
        "paths",
        "edits",
        "items",
        "matches",
        "results",
        "steps",
        "replacements",
        "changes",
    ];
    let lower = name.to_lowercase();
    SUFFIXES.iter().any(|s| lower.ends_with(s) || lower == *s)
}

// ---------------------------------------------------------------------------
// Repair #4: Wrap bare string as array
// ---------------------------------------------------------------------------

/// If a field should be an array but the model sent a bare string,
/// wrap it in a single-element array.
fn wrap_bare_string_as_array(v: &mut Value) {
    if let Value::Object(map) = v {
        for (key, val) in map.iter_mut() {
            if val.is_string() && looks_like_array_field(key) {
                let s = std::mem::take(val);
                *val = Value::Array(vec![s]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Repair #5: Sanitize embedded markdown auto-link leakage in path fields
// ---------------------------------------------------------------------------

/// DeepSeek sometimes emits file paths with embedded markdown auto-links:
/// `/Users/x/proj/[notes.md](http://notes.md)` instead of `/Users/x/proj/notes.md`.
///
/// This is the post-training chat distribution leaking through the tool
/// boundary — the model has been rewarded for auto-linking in conversational
/// output, and applies that prior where it makes no sense.
///
/// The fix detects `[text](http://text)` patterns within the string and
/// replaces them with just `text`. Real markdown links like
/// `[click](https://x.com)` are preserved because the link text differs
/// from the URL-without-protocol.
fn sanitize_markdown_autolink_paths(v: &mut Value) {
    if let Value::Object(map) = v {
        for (key, val) in map.iter_mut() {
            if val.is_string() && is_path_field(key) {
                let Value::String(s) = val else { continue };
                *s = strip_embedded_markdown_autolinks(s);
            }
        }
    }
}

/// Does this field name look like it holds a path?
fn is_path_field(name: &str) -> bool {
    const PATH_NAMES: &[&str] = &[
        "path",
        "filepath",
        "file_path",
        "source",
        "destination",
        "dest",
        "from",
        "to",
        "dir",
        "directory",
        "folder",
        "file",
        "filename",
        "input_file",
        "output_file",
        "old_path",
        "new_path",
    ];
    let lower = name.to_lowercase();
    PATH_NAMES
        .iter()
        .any(|p| lower == *p || lower.ends_with("_path"))
}

/// Strip embedded markdown auto-link patterns from a path string.
///
/// `/Users/x/proj/[notes.md](http://notes.md)` → `/Users/x/proj/notes.md`
/// `[notes.md](http://notes.md)` → `notes.md`
/// `[click](https://x.com)` → `[click](https://x.com)` (unchanged)
fn strip_embedded_markdown_autolinks(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Try to parse as markdown link: [text](url)
            let Some(close_bracket) = s[i + 1..].find(']') else {
                result.push(bytes[i] as char);
                i += 1;
                continue;
            };
            let after_bracket = i + 1 + close_bracket + 1;
            if after_bracket >= s.len() || !s[after_bracket..].starts_with('(') {
                result.push(bytes[i] as char);
                i += 1;
                continue;
            }
            let Some(close_paren) = s[after_bracket + 1..].find(')') else {
                result.push(bytes[i] as char);
                i += 1;
                continue;
            };
            let text = &s[i + 1..i + 1 + close_bracket];
            let url = &s[after_bracket + 1..after_bracket + 1 + close_paren];
            let url_path = url
                .strip_prefix("http://")
                .or_else(|| url.strip_prefix("https://"))
                .unwrap_or(url);
            if text == url_path {
                // Degenerate autolink — strip it
                result.push_str(text);
                i = after_bracket + 1 + close_paren + 1;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the index of the last `}` or `]` that balances with an earlier
/// opening brace/bracket.
fn find_last_balanced_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut last_close = None;
    let mut in_string = false;
    let mut escape = false;
    let mut in_bracket = 0i32;

    for (i, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 && in_bracket == 0 {
                    last_close = Some(i);
                }
            }
            '[' => in_bracket += 1,
            ']' => {
                in_bracket -= 1;
                if in_bracket == 0 && depth == 0 {
                    last_close = Some(i);
                }
            }
            _ => {}
        }
    }

    last_close
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_input_unchanged() {
        let input = json!({"path": "src/main.rs", "content": "hello"});
        let repaired = repair_value(&input);
        assert_eq!(input, repaired);
    }

    #[test]
    fn null_optional_fields_stripped() {
        let input = json!({"path": "src/main.rs", "content": null, "extra": null});
        let repaired = repair_value(&input);
        assert_eq!(repaired, json!({"path": "src/main.rs"}));
    }

    #[test]
    fn stringified_array_parsed() {
        let input = json!({
            "path": "src/main.rs",
            "edits": r#"[{"old_text":"a","new_text":"b"}]"#
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["edits"], json!([{"old_text":"a","new_text":"b"}]));
    }

    #[test]
    fn stringified_object_parsed() {
        let input = json!({
            "path": "src/main.rs",
            "config": r#"{"key":"value"}"#
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["config"], json!({"key":"value"}));
    }

    #[test]
    fn bare_object_wrapped_as_array() {
        let input = json!({
            "path": "src/main.rs",
            "edits": {"old_text": "a", "new_text": "b"}
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["edits"], json!([{"old_text":"a","new_text":"b"}]));
    }

    #[test]
    fn bare_string_wrapped_as_array() {
        let input = json!({
            "path": "src/main.rs",
            "edits": "hello"
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["edits"], json!(["hello"]));
    }

    #[test]
    fn markdown_autolink_stripped() {
        let input = json!({
            "path": "/Users/x/proj/[notes.md](http://notes.md)"
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["path"], json!("/Users/x/proj/notes.md"));
    }

    #[test]
    fn markdown_autolink_stripped_whole_path() {
        let input = json!({
            "path": "[notes.md](http://notes.md)"
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["path"], json!("notes.md"));
    }

    #[test]
    fn real_markdown_link_untouched() {
        let input = json!({
            "path": "[click](https://example.com)"
        });
        let repaired = repair_value(&input);
        // "click" != "example.com", so this should be untouched.
        assert_eq!(repaired["path"], json!("[click](https://example.com)"));
    }

    #[test]
    fn normal_path_untouched() {
        let input = json!({"path": "src/main.rs"});
        let repaired = repair_value(&input);
        assert_eq!(repaired["path"], json!("src/main.rs"));
    }

    #[test]
    fn multiple_repairs_applied_together() {
        let input = json!({
            "path": "/Users/x/proj/[notes.md](http://notes.md)",
            "content": null,
            "edits": r#"[{"old_text":"foo","new_text":"bar"}]"#
        });
        let repaired = repair_value(&input);
        assert_eq!(repaired["path"], json!("/Users/x/proj/notes.md"));
        assert_eq!(repaired.get("content"), None);
        assert_eq!(
            repaired["edits"],
            json!([{"old_text":"foo","new_text":"bar"}])
        );
    }

    #[test]
    fn stringified_array_not_double_wrapped() {
        // JSON-array-parse must run before bare-string-wrap,
        // or '["a","b"]' becomes ['["a","b"]'].
        let input = json!({
            "edits": r#"["a","b"]"#
        });
        let repaired = repair_value(&input);
        // Should be parsed as an array, not wrapped
        assert_eq!(repaired["edits"], json!(["a", "b"]));
    }

    #[test]
    fn repair_json_string_valid() {
        let raw = r#"{"path": "src/main.rs"}"#;
        let result = repair_json_string(raw);
        assert_eq!(result, Some(json!({"path": "src/main.rs"})));
    }

    #[test]
    fn repair_json_string_trailing_comma() {
        let raw = r#"{"path": "src/main.rs",}"#;
        let result = repair_json_string(raw);
        assert_eq!(result, Some(json!({"path": "src/main.rs"})));
    }

    #[test]
    fn repair_json_string_whitespace() {
        let raw = "  {\"path\": \"src/main.rs\"}  \n  ";
        let result = repair_json_string(raw);
        assert_eq!(result, Some(json!({"path": "src/main.rs"})));
    }

    #[test]
    fn repair_json_string_truncated_finds_balanced() {
        // Simulates stream appending garbage after a valid JSON object.
        let raw = r#"{"path": "src/main.rs"}extra garbage{"#;
        let result = repair_json_string(raw);
        assert_eq!(result, Some(json!({"path": "src/main.rs"})));
    }

    #[test]
    fn repair_json_string_unparseable_returns_none() {
        let raw = "this is not json at all";
        assert_eq!(repair_json_string(raw), None);
    }
}
