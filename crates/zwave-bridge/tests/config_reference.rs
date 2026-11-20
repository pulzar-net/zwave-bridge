//! The configuration reference on the docs site is generated from the
//! configuration types, so it cannot drift from the code. This test fails
//! when the committed reference (or the JSON Schema next to it) differs from
//! what the types say; regenerate both with
//!
//! ```sh
//! UPDATE_DOCS=1 cargo test -p zwave-bridge --features schema --test config_reference
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Map, Value};

use zwave_bridge::config::Config;

const PAGE: &str = "docs/src/config-reference.md";
const SCHEMA: &str = "docs/src/config.schema.json";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Follow a `$ref` into `$defs`.
fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(r) => {
            let name = r.trim_start_matches("#/$defs/");
            &root["$defs"][name]
        }
        None => schema,
    }
}

/// The schema without its `null` alternative, and whether it had one.
fn strip_null<'a>(root: &'a Value, schema: &'a Value) -> (&'a Value, bool) {
    if let Some(any) = schema.get("anyOf").and_then(Value::as_array) {
        let non_null: Vec<&Value> = any
            .iter()
            .filter(|s| s.get("type") != Some(&Value::from("null")))
            .collect();
        if non_null.len() == 1 && any.len() == 2 {
            return (resolve(root, non_null[0]), true);
        }
    }
    let nullable = matches!(schema.get("type"), Some(Value::Array(t)) if t.contains(&Value::from("null")));
    (resolve(root, schema), nullable)
}

fn is_table(schema: &Value) -> bool {
    schema.get("properties").is_some()
}

fn variants(schema: &Value) -> Option<Vec<(String, Option<String>)>> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Some(values.iter().map(|v| (v.to_string(), None)).collect());
    }
    let one_of = schema.get("oneOf").and_then(Value::as_array)?;
    one_of
        .iter()
        .map(|v| {
            let value = v
                .get("const")
                .cloned()
                .or_else(|| v.get("enum").and_then(|e| e.get(0)).cloned())?;
            let doc = v.get("description").and_then(Value::as_str).map(str::to_string);
            Some((value.to_string(), doc))
        })
        .collect()
}

fn type_name(root: &Value, schema: &Value) -> String {
    let (schema, _) = strip_null(root, schema);
    if let Some(vs) = variants(schema) {
        return vs.iter().map(|(v, _)| format!("`{v}`")).collect::<Vec<_>>().join(", ");
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("any"),
        _ => "any",
    };
    match ty {
        "array" => match schema.get("items") {
            Some(items) => format!("array of {}", type_name(root, items)),
            None => "array".into(),
        },
        "object" => match schema.get("additionalProperties") {
            Some(v) if v.is_object() => format!("table of {}", type_name(root, v)),
            _ => "table".into(),
        },
        "integer" => {
            let min = schema.get("minimum").and_then(Value::as_i64);
            let max = schema.get("maximum").and_then(Value::as_i64);
            match (min, max) {
                (Some(lo), Some(hi)) => format!("integer ({lo} to {hi})"),
                (Some(lo), None) => format!("integer (from {lo})"),
                _ => "integer".into(),
            }
        }
        other => other.into(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").replace('|', "\\|")
}

fn toml_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(toml_value).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) if m.is_empty() => "{}".into(),
        other => other.to_string(),
    }
}

/// One section per table, depth first, in declaration order.
fn section(
    root: &Value,
    path: &str,
    array: bool,
    schema: &Value,
    required_here: bool,
    out: &mut String,
) {
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let header = if array {
        format!("[[{path}]]")
    } else {
        format!("[{path}]")
    };
    let _ = writeln!(out, "## `{header}`\n");
    if let Some(d) = schema.get("description").and_then(Value::as_str) {
        let _ = writeln!(out, "{}", one_line(d));
    }
    if array {
        let _ = writeln!(out, "\nA list: repeat the `{header}` table once per entry.");
    } else if required_here {
        let _ = writeln!(out, "\nThis table is required.");
    }
    let _ = writeln!(out);
    let mut nested: Vec<(String, bool, Value, bool)> = Vec::new();
    let mut rows = String::new();
    for (key, prop) in &props {
        let (inner, nullable) = strip_null(root, prop);
        let child = format!("{path}.{key}");
        if is_table(inner) {
            nested.push((child, false, inner.clone(), required.contains(&key.as_str())));
            continue;
        }
        if inner.get("type") == Some(&Value::from("array")) {
            if let Some(items) = inner.get("items") {
                let item = resolve(root, items);
                if is_table(item) {
                    nested.push((child, true, item.clone(), false));
                    continue;
                }
            }
        }
        let default = if required.contains(&key.as_str()) {
            "required".to_string()
        } else {
            match prop.get("default").or_else(|| schema.get("default").and_then(|d| d.get(key))) {
                Some(Value::Null) | None if nullable => "unset".into(),
                Some(v) => format!("`{}`", toml_value(v)),
                None => "unset".into(),
            }
        };
        let mut doc = prop
            .get("description")
            .or_else(|| inner.get("description"))
            .and_then(Value::as_str)
            .map(one_line)
            .unwrap_or_default();
        if let Some(vs) = variants(inner) {
            let docs: Vec<String> = vs
                .iter()
                .filter_map(|(v, d)| d.as_ref().map(|d| format!("`{v}`: {}", one_line(d))))
                .collect();
            if !docs.is_empty() {
                doc = format!("{doc} {}", docs.join(" "));
            }
        }
        let _ = writeln!(
            rows,
            "| `{key}` | {} | {default} | {} |",
            type_name(root, prop),
            doc.trim()
        );
    }
    if !rows.is_empty() {
        let _ = writeln!(out, "| Key | Type | Default | Description |");
        let _ = writeln!(out, "|---|---|---|---|");
        let _ = writeln!(out, "{rows}");
    }
    for (child, is_array, schema, req) in nested {
        section(root, &child, is_array, &schema, req, out);
    }
}

fn render(schema: &Value) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Configuration reference\n");
    let _ = writeln!(
        out,
        "<!-- Generated from the configuration types by crates/zwave-bridge/tests/config_reference.rs. Do not edit by hand. -->\n"
    );
    let _ = writeln!(
        out,
        "Every key the configuration file accepts. Unknown keys are an error, so a typo stops the bridge at startup instead of being ignored. \
         This page is generated from the configuration types; a [JSON Schema](config.schema.json) generated from the same types \
         lets editors with TOML schema support (for example Taplo or Even Better TOML) complete and check the file.\n"
    );
    let props: Map<String, Value> = schema["properties"].as_object().cloned().unwrap_or_default();
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (key, prop) in &props {
        let (inner, _) = strip_null(schema, prop);
        if is_table(inner) {
            section(schema, key, false, inner, required.contains(&key.as_str()), &mut out);
        } else if inner.get("type") == Some(&Value::from("array")) {
            let item = resolve(schema, &inner["items"]);
            section(schema, key, true, item, false, &mut out);
        }
    }
    out.trim_end().to_string() + "\n"
}

#[test]
fn config_reference_is_current() {
    let schema = schemars::schema_for!(Config);
    let schema = serde_json::to_value(&schema).unwrap();
    let page = render(&schema);
    let json = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    let root = repo_root();
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(root.join(PAGE), &page).unwrap();
        std::fs::write(root.join(SCHEMA), &json).unwrap();
        return;
    }
    let stale = "is stale: regenerate it with \
                 UPDATE_DOCS=1 cargo test -p zwave-bridge --features schema --test config_reference";
    let committed = std::fs::read_to_string(root.join(PAGE)).unwrap_or_default();
    assert!(committed == page, "{PAGE} {stale}");
    let committed = std::fs::read_to_string(root.join(SCHEMA)).unwrap_or_default();
    assert!(committed == json, "{SCHEMA} {stale}");
}

#[test]
fn example_config_parses() {
    let text = std::fs::read_to_string(repo_root().join("config.example.toml")).unwrap();
    Config::parse(&text).expect("config.example.toml");
}
