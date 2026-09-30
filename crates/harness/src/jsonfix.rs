//! Best-effort repair for truncated JSON (cut streaming tool-call args).
//! If the input already parses, it passes through untouched.

/// Close open strings/objects/arrays; drop a dangling comma or colon first.
pub fn repair_json(partial: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(partial).is_ok() {
        return partial.to_string();
    }
    let mut out = String::with_capacity(partial.len() + 8);
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    for c in partial.chars() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' => {
                stack.push('}');
                out.push(c);
            }
            '[' => {
                stack.push(']');
                out.push(c);
            }
            '}' | ']' => {
                if stack.last() == Some(&c) {
                    stack.pop();
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    if in_string {
        out.push('"');
    }
    // A trailing comma or colon would invalidate the next closer.
    while out.ends_with(',') || out.ends_with(':') {
        out.pop();
    }
    // A dangling `"key"` (not `"key": value`) needs `:null` to close.
    let trimmed = out.trim_end().to_string();
    let mut out = trimmed;
    if out.ends_with('"') && stack.last() == Some(&'}') {
        if let Some(key_start) = out.trim_end_matches('"').rfind('"') {
            let before = out[..key_start].trim_end();
            if before.ends_with('{') || before.ends_with(',') {
                out.push_str(":null");
            }
        }
    }
    for closer in stack.iter().rev() {
        out.push(*closer);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_passes_through() {
        assert_eq!(repair_json(r#"{"a":[1,2]}"#), r#"{"a":[1,2]}"#);
    }

    #[test]
    fn truncated_object_and_array_close() {
        let fixed = repair_json(r#"{"path": "a.rs", "options": ["x", "y"#);
        let v: serde_json::Value = serde_json::from_str(&fixed).unwrap();
        assert_eq!(v["options"], serde_json::json!(["x", "y"]));
    }

    #[test]
    fn cut_string_and_dangling_comma_repaired() {
        let fixed = repair_json(r#"{"a": 1, "b": "hel"#);
        let v: serde_json::Value = serde_json::from_str(&fixed).unwrap();
        assert_eq!(v["b"], serde_json::json!("hel"));
        let fixed = repair_json(r#"{"a": 1,"#);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok());
        // A complete string value must not gain a phantom key.
        let fixed = repair_json(r#"{"a": "b"#);
        let v: serde_json::Value = serde_json::from_str(&fixed).unwrap();
        assert_eq!(v["a"], serde_json::json!("b"));
        let fixed = repair_json(r#"{"a"#);
        let v: serde_json::Value = serde_json::from_str(&fixed).unwrap();
        assert!(v.get("a").is_some());
    }
}
