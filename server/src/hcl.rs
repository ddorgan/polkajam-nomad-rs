use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ParsedHcl {
    pub optional: Vec<String>,
    pub required: Vec<String>,
    pub defaults: Map<String, Value>,
    pub job_name: Option<String>,
    pub count: Option<u32>,
}

pub fn parse_hcl(text: &str) -> ParsedHcl {
    let optional = meta_list(text, r"meta_optional\s*=\s*\[(.*?)\]");
    let required = meta_list(text, r"meta_required\s*=\s*\[(.*?)\]");

    let count = Regex::new(r"(?m)^\s*count\s*=\s*(\d+)")
        .ok()
        .and_then(|re| re.captures(text))
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok());

    let mut defaults = Map::new();
    if let Ok(meta_re) = Regex::new(r"meta\s*\{([^{}]*)\}") {
        for cap in meta_re.captures_iter(text) {
            if let Some(body) = cap.get(1) {
                merge_defaults(body.as_str(), &mut defaults);
            }
        }
    }

    let job_name = Regex::new(r#"(?m)^\s*job\s+"([^"]+)""#)
        .ok()
        .and_then(|re| re.captures(text))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    ParsedHcl {
        optional,
        required,
        defaults,
        job_name,
        count,
    }
}

pub fn parse_hcl_file(path: &Path) -> std::io::Result<ParsedHcl> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse_hcl(&text))
}

pub fn allowed_meta(parsed: &ParsedHcl) -> HashSet<String> {
    parsed
        .required
        .iter()
        .chain(parsed.optional.iter())
        .cloned()
        .collect()
}

pub fn filter_meta(meta: &Map<String, Value>, allowed: &HashSet<String>) -> Map<String, Value> {
    meta.iter()
        .filter(|(k, v)| allowed.contains(k.as_str()) && !value_is_empty(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn write_count_variant(
    src: &Path,
    dst: &Path,
    new_count: u32,
    new_job_name: &str,
) -> Result<(), String> {
    let text = std::fs::read_to_string(src).map_err(|e| e.to_string())?;

    let job_re = Regex::new(r#"(?m)^(\s*job\s+")[^"]+(")"#).map_err(|e| e.to_string())?;
    if !job_re.is_match(&text) {
        return Err(format!("could not find job stanza in {}", src.display()));
    }
    let text = job_re
        .replacen(&text, 1, format!("${{1}}{new_job_name}${{2}}"))
        .into_owned();

    let count_re = Regex::new(r"(?m)^(\s*count\s*=\s*)\d+").map_err(|e| e.to_string())?;
    if !count_re.is_match(&text) {
        return Err(format!("could not find group count in {}", src.display()));
    }
    let text = count_re
        .replacen(&text, 1, format!("${{1}}{new_count}"))
        .into_owned();

    std::fs::write(dst, text).map_err(|e| e.to_string())
}

fn meta_list(text: &str, pattern: &str) -> Vec<String> {
    Regex::new(pattern)
        .ok()
        .and_then(|re| re.captures(text))
        .and_then(|c| c.get(1))
        .map(|m| {
            Regex::new(r#""([^"]+)""#)
                .unwrap()
                .captures_iter(m.as_str())
                .filter_map(|cap| cap.get(1).map(|s| s.as_str().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn merge_defaults(body: &str, defaults: &mut Map<String, Value>) {
    let kv_re = Regex::new(r"([A-Za-z_][\w]*)\s*=\s*(.+)").unwrap();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(cap) = kv_re.captures(line) else {
            continue;
        };
        let key = cap.get(1).unwrap().as_str();
        let raw = cap.get(2).unwrap().as_str().trim();
        defaults.insert(key.to_string(), parse_meta_value(raw));
    }
}

fn parse_meta_value(raw: &str) -> Value {
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return Value::String(raw[1..raw.len() - 1].to_string());
    }
    if raw == "true" {
        return Value::Bool(true);
    }
    if raw == "false" {
        return Value::Bool(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return json!(n);
    }
    if let Ok(n) = raw.parse::<f64>() {
        return json!(n);
    }
    Value::String(raw.to_string())
}

fn value_is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}
