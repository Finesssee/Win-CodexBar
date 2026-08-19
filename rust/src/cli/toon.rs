use serde_json::{Map, Value};

/// TOON v4.1-compatible presentation formatter for CLI JSON payloads.
/// It is presentation-only: callers pass the exact JSON value they would otherwise emit.
pub fn encode(value: &Value) -> String {
    render_value(None, value, 0).join("\n")
}

fn render_value(key: Option<&str>, value: &Value, indent: usize) -> Vec<String> {
    match value {
        Value::Null => vec![scalar_line(key, "null", indent)],
        Value::Bool(value) => vec![scalar_line(key, if *value { "true" } else { "false" }, indent)],
        Value::Number(value) => vec![scalar_line(key, &value.to_string(), indent)],
        Value::String(value) => vec![scalar_line(key, &quote_if_needed(value), indent)],
        Value::Array(items) => render_array(key, items, indent),
        Value::Object(entries) => render_object(key, entries, indent),
    }
}

fn scalar_line(key: Option<&str>, literal: &str, indent: usize) -> String {
    let prefix = pad(indent);
    match key {
        Some(key) => format!("{prefix}{}: {literal}", quote_key_if_needed(key)),
        None => format!("{prefix}{literal}"),
    }
}

fn render_object(key: Option<&str>, entries: &Map<String, Value>, indent: usize) -> Vec<String> {
    if entries.is_empty() {
        return match key {
            Some(key) => vec![format!("{}{}:", pad(indent), quote_key_if_needed(key))],
            None => vec![format!("{}{{}}", pad(indent))],
        };
    }

    let mut lines = Vec::new();
    let child_indent = if let Some(key) = key {
        lines.push(format!("{}{}:", pad(indent), quote_key_if_needed(key)));
        indent + 1
    } else {
        indent
    };
    for (child_key, child) in entries {
        lines.extend(render_value(Some(child_key), child, child_indent));
    }
    lines
}

fn render_array(key: Option<&str>, items: &[Value], indent: usize) -> Vec<String> {
    let prefix = pad(indent);
    let header = key.map(quote_key_if_needed).unwrap_or_default();
    if items.is_empty() {
        return vec![if key.is_some() {
            format!("{prefix}{header}: []")
        } else {
            format!("{prefix}[]")
        }];
    }

    if items.iter().all(is_scalar) {
        let values = items.iter().map(scalar_literal).collect::<Vec<_>>().join(",");
        return vec![format!("{prefix}{header}[{}]: {values}", items.len())];
    }

    if let Some(fields) = tabular_fields(items) {
        let field_list = fields
            .iter()
            .map(|field| quote_key_if_needed(field))
            .collect::<Vec<_>>()
            .join(",");
        let mut lines = vec![format!("{prefix}{header}[{}]{{{field_list}}}:", items.len())];
        for item in items {
            let Value::Object(object) = item else { continue };
            let cells = fields
                .iter()
                .map(|field| scalar_literal(object.get(field).unwrap_or(&Value::Null)))
                .collect::<Vec<_>>();
            lines.push(format!("{}{}", pad(indent + 1), cells.join(",")));
        }
        return lines;
    }

    let mut lines = vec![format!("{prefix}{header}[{}]:", items.len())];
    for item in items {
        lines.extend(render_list_item(item, indent + 1));
    }
    lines
}

fn render_list_item(value: &Value, indent: usize) -> Vec<String> {
    match value {
        Value::Object(entries) if !entries.is_empty() => {
            let mut iter = entries.iter();
            let (first_key, first_value) = iter.next().expect("non-empty object");
            let mut first_lines = render_value(Some(first_key), first_value, indent + 1);
            let first = first_lines.remove(0);
            let strip = (indent + 1) * 2;
            let content = first.get(strip..).unwrap_or(&first);
            let mut lines = vec![format!("{}- {content}", pad(indent))];
            lines.extend(first_lines);
            for (key, value) in iter {
                lines.extend(render_value(Some(key), value, indent + 1));
            }
            lines
        }
        Value::Array(items) => {
            let mut rendered = render_array(None, items, indent + 1);
            if rendered.is_empty() {
                return vec![format!("{}-", pad(indent))];
            }
            let first = rendered.remove(0);
            let strip = (indent + 1) * 2;
            let content = first.get(strip..).unwrap_or(&first);
            let mut lines = vec![format!("{}- {content}", pad(indent))];
            lines.extend(rendered);
            lines
        }
        _ => vec![format!("{}- {}", pad(indent), scalar_literal(value))],
    }
}

fn tabular_fields(items: &[Value]) -> Option<Vec<String>> {
    let Value::Object(first) = items.first()? else { return None };
    if first.is_empty() || !first.values().all(is_scalar) {
        return None;
    }
    let fields = first.keys().cloned().collect::<Vec<_>>();
    for item in items.iter().skip(1) {
        let Value::Object(object) = item else { return None };
        if object.len() != fields.len()
            || !fields.iter().zip(object.keys()).all(|(left, right)| left == right)
            || !object.values().all(is_scalar)
        {
            return None;
        }
    }
    Some(fields)
}

fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_))
}

fn scalar_literal(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => if *value { "true" } else { "false" }.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => quote_if_needed(value),
        _ => "null".to_string(),
    }
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

fn quote_key_if_needed(key: &str) -> String {
    let mut chars = key.chars();
    let valid_first = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_');
    if valid_first && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.') {
        key.to_string()
    } else {
        format!("\"{}\"", escape(key))
    }
}

fn quote_if_needed(value: &str) -> String {
    if needs_quoting(value) {
        format!("\"{}\"", escape(value))
    } else {
        value.to_string()
    }
}

fn needs_quoting(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with(' ')
        || value.ends_with(' ')
        || matches!(value, "true" | "false" | "null")
        || value.parse::<f64>().is_ok()
        || value.starts_with('-')
        || value.starts_with('#')
    {
        return true;
    }
    value.chars().any(|ch| {
        matches!(ch, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | ',') || ch.is_control()
    })
}

fn escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_uniform_objects_as_table() {
        let value = json!([
            { "provider": "codex", "used": 10 },
            { "provider": "claude", "used": 20 }
        ]);
        assert_eq!(encode(&value), "[2]{provider,used}:\n  codex,10\n  claude,20");
    }

    #[test]
    fn preserves_nonuniform_object_fields_as_list() {
        let value = json!([
            { "provider": "codex", "plan": "plus" },
            { "provider": "claude" }
        ]);
        let encoded = encode(&value);
        assert!(encoded.contains("- plan: plus") || encoded.contains("plan: plus"));
        assert!(encoded.contains("provider: claude"));
    }

    #[test]
    fn quotes_numeric_like_strings() {
        assert_eq!(encode(&json!("123")), "\"123\"");
    }
}
