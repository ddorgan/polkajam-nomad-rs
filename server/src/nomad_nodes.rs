use serde::Serialize;
use serde_json::{Map, Value};

use crate::nomad::nomad_addr;

/// Default dynamic meta `role` for chainspec / Nomad host selection.
pub const CHAIN_NOMAD_META_ROLE: &str = "validators";

/// Resolved meta role: env `NOMAD_CHAIN_META_ROLE`, default [`CHAIN_NOMAD_META_ROLE`].
/// Accepts legacy `validator` as an alias for `validators`.
pub fn chain_nomad_meta_role() -> String {
    normalize_meta_role(
        std::env::var("NOMAD_CHAIN_META_ROLE").unwrap_or_else(|_| CHAIN_NOMAD_META_ROLE.into()),
    )
}

pub fn normalize_meta_role(role: impl AsRef<str>) -> String {
    let role = role.as_ref().trim();
    if role.is_empty() {
        return CHAIN_NOMAD_META_ROLE.to_string();
    }
    if role.eq_ignore_ascii_case("validator") {
        return CHAIN_NOMAD_META_ROLE.to_string();
    }
    role.to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NomadHost {
    pub id: String,
    pub name: String,
    pub status: String,
    pub datacenter: String,
    pub role: String,
    pub client_ip: String,
}

#[derive(Debug, Default)]
struct ScanStats {
    total: u32,
    ready: u32,
    matched: u32,
    with_ip: u32,
}

pub async fn fetch_nomad_nodes_raw() -> Result<Value, String> {
    nomad_get("/v1/nodes").await
}

async fn fetch_node_detail(node_id: &str) -> Result<Value, String> {
    nomad_get(&format!("/v1/node/{}", node_id)).await
}

/// Dynamic node metadata from `GET /v1/client/metadata?node_id=…` (not merged `Meta` on `/v1/nodes`).
async fn fetch_node_dynamic_metadata(node_id: &str) -> Result<Value, String> {
    nomad_get(&format!(
        "/v1/client/metadata?node_id={}",
        urlencoding_encode(node_id)
    ))
    .await
}

fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

async fn nomad_get(path: &str) -> Result<Value, String> {
    let base = nomad_addr().trim_end_matches('/').to_string();
    let url = format!("{base}{path}");

    let mut req = reqwest::Client::new().get(&url);
    if let Ok(token) = std::env::var("NOMAD_TOKEN") {
        if !token.is_empty() {
            req = req.header("X-Nomad-Token", token);
        }
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Nomad API error {status} for {path}: {text}"));
    }
    resp.json().await.map_err(|e| e.to_string())
}

fn preferred_ip_prefix() -> String {
    std::env::var("NOMAD_HOST_IP_PREFIX").unwrap_or_else(|_| "192.168.20.".into())
}

fn json_str(obj: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = obj.get(*key).and_then(|v| v.as_str()) {
            let s = v.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn json_bool(obj: &Value, keys: &[&str]) -> bool {
    for key in keys {
        if let Some(v) = obj.get(*key).and_then(|v| v.as_bool()) {
            return v;
        }
    }
    false
}

/// Parsed dynamic metadata attached to a node candidate during host scans.
fn dynamic_meta_map(node: &Value) -> Option<&Map<String, Value>> {
    node.get("DynamicMeta")
        .or_else(|| node.get("dynamic_meta"))
        .or_else(|| node.get("Dynamic"))
        .or_else(|| node.get("dynamic"))
        .and_then(|v| v.as_object())
}

fn attrs_map(node: &Value) -> Option<&Map<String, Value>> {
    node.get("Attributes")
        .or_else(|| node.get("attributes"))
        .and_then(|v| v.as_object())
}

fn meta_value(meta: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = meta.get(*key).and_then(|v| v.as_str()) {
            let s = v.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn attr_value(attrs: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = attrs.get(*key).and_then(|v| v.as_str()) {
            let s = v.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn looks_like_ipv4(ip: &str) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u16>().ok().is_some_and(|n| n <= 255))
}

fn ipv4s_from_list(raw: &str) -> Vec<String> {
    raw.split([',', ' '])
        .map(str::trim)
        .filter(|s| looks_like_ipv4(s))
        .map(str::to_string)
        .collect()
}

fn pick_preferred_ipv4(candidates: &[String], prefix: &str) -> Option<String> {
    if let Some(ip) = candidates.iter().find(|ip| ip.starts_with(prefix)) {
        return Some(ip.clone());
    }
    candidates.first().cloned()
}

pub fn node_client_ip(node: &Value) -> Option<String> {
    node_client_ip_with_prefix(node, &preferred_ip_prefix())
}

pub fn node_client_ip_with_prefix(node: &Value, prefix: &str) -> Option<String> {
    if let Some(meta) = dynamic_meta_map(node) {
        if let Some(ip) = meta_value(
            meta,
            &["client_ip", "client-ip", "clientip", "clientIp"],
        ) {
            if looks_like_ipv4(&ip) {
                return Some(ip);
            }
        }
    }

    if let Some(attrs) = attrs_map(node) {
        if let Some(ip) = attr_value(attrs, &["meta.client_ip", "meta.client-ip"]) {
            if looks_like_ipv4(&ip) {
                return Some(ip);
            }
        }

        let mut network_ips = Vec::new();
        for key in [
            "unique.network.ip-addresses",
            "unique.network.ip-address",
            "network.ip-address",
        ] {
            if let Some(raw) = attrs.get(key).and_then(|v| v.as_str()) {
                network_ips.extend(ipv4s_from_list(raw));
            }
        }
        if let Some(ip) = pick_preferred_ipv4(&network_ips, prefix) {
            return Some(ip);
        }
    }

    for key in &["Address", "address", "HttpAddr", "http_addr"] {
        if let Some(ip) = json_str(node, &[*key]) {
            if looks_like_ipv4(&ip) {
                return Some(ip);
            }
            // HttpAddr can be "IP:PORT"
            if let Some(host) = ip.split(':').next() {
                if looks_like_ipv4(host) {
                    return Some(host.to_string());
                }
            }
        }
    }

    None
}

pub fn node_role(node: &Value) -> Option<String> {
    dynamic_meta_map(node).and_then(|meta| meta_value(meta, &["role"]))
}

pub fn node_is_available(node: &Value) -> bool {
    let status = json_str(node, &["Status", "status"]).unwrap_or_default();
    if !status.eq_ignore_ascii_case("ready") {
        return false;
    }
    if json_bool(node, &["Drain", "drain"]) {
        return false;
    }
    let scheduling = json_str(node, &["SchedulingEligibility", "scheduling_eligibility"]);
    if let Some(eligibility) = scheduling {
        if !eligibility.eq_ignore_ascii_case("eligible") {
            return false;
        }
    }
    true
}

pub fn node_role_matches(node: &Value, role: &str) -> bool {
    node_role(node).as_deref() == Some(role)
}

fn merge_node_detail(stub: &mut Value, detail: &Value) {
    let Some(obj) = stub.as_object_mut() else {
        return;
    };
    for key in ["Attributes", "attributes"] {
        if let Some(v) = detail.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }
}

fn parse_dynamic_meta_map(resp: &Value) -> Map<String, Value> {
    let Some(dynamic) = resp.get("Dynamic").or_else(|| resp.get("dynamic")) else {
        return Map::new();
    };
    let Some(obj) = dynamic.as_object() else {
        return Map::new();
    };
    let mut out = Map::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        if let Some(s) = value.as_str() {
            let s = s.trim();
            if !s.is_empty() {
                out.insert(key.clone(), Value::String(s.to_string()));
            }
        }
    }
    out
}

fn attach_dynamic_meta(stub: &mut Value, dynamic: Map<String, Value>) {
    if dynamic.is_empty() {
        return;
    }
    let Some(obj) = stub.as_object_mut() else {
        return;
    };
    obj.insert("DynamicMeta".to_string(), Value::Object(dynamic));
}

async fn enrich_node_candidate(stub: &mut Value, prefix: &str) {
    let Some(id) = json_str(stub, &["ID", "id"]) else {
        return;
    };

    if let Ok(resp) = fetch_node_dynamic_metadata(&id).await {
        let dynamic = parse_dynamic_meta_map(&resp);
        attach_dynamic_meta(stub, dynamic);
    }

    if node_client_ip_with_prefix(stub, prefix).is_none() {
        if let Ok(detail) = fetch_node_detail(&id).await {
            merge_node_detail(stub, &detail);
        }
    }
}

pub fn parse_nomad_host(node: &Value, role: &str) -> Option<NomadHost> {
    parse_nomad_host_with_prefix(node, role, &preferred_ip_prefix())
}

fn parse_nomad_host_with_prefix(node: &Value, role: &str, prefix: &str) -> Option<NomadHost> {
    if !node_is_available(node) || !node_role_matches(node, role) {
        return None;
    }
    let client_ip = node_client_ip_with_prefix(node, prefix)?;
    Some(nomad_host_from_node(node, role, client_ip))
}

fn nomad_host_from_node(node: &Value, role: &str, client_ip: String) -> NomadHost {
    NomadHost {
        id: json_str(node, &["ID", "id"]).unwrap_or_default(),
        name: json_str(node, &["Name", "name"]).unwrap_or_default(),
        status: json_str(node, &["Status", "status"]).unwrap_or_default(),
        datacenter: json_str(node, &["Datacenter", "datacenter"]).unwrap_or_default(),
        role: role.to_string(),
        client_ip,
    }
}

fn empty_hosts_error(meta_role: &str, stats: &ScanStats, prefix: &str) -> String {
    format!(
        "no available Nomad hosts with dynamic meta role={meta_role} (status=ready, eligible, not draining, with host IP). \
         Scanned {} node(s): {} ready, {} with dynamic meta role={meta_role}, {} with a host IP. \
         Set dynamic metadata via `nomad node meta apply` (role={meta_role}, client_ip, …; or a {prefix}* network address). \
         Preview with: deploy-cli chain hosts --role {meta_role}",
        stats.total, stats.ready, stats.matched, stats.with_ip
    )
}

pub async fn available_nomad_hosts(meta_role: &str) -> Result<Vec<NomadHost>, String> {
    let meta_role = normalize_meta_role(meta_role);
    if meta_role.is_empty() {
        return Err("meta.role must not be empty".into());
    }
    let meta_role = meta_role.as_str();

    let prefix = preferred_ip_prefix();
    let nodes = fetch_nomad_nodes_raw().await?;
    let items = nodes
        .as_array()
        .ok_or_else(|| "Nomad /v1/nodes did not return a JSON array".to_string())?;

    let mut hosts = Vec::new();
    let mut stats = ScanStats::default();

    for node in items {
        stats.total += 1;
        if !node_is_available(node) {
            continue;
        }
        stats.ready += 1;

        let mut candidate = node.clone();
        enrich_node_candidate(&mut candidate, &prefix).await;

        if node_role_matches(&candidate, meta_role) {
            stats.matched += 1;
        }
        if node_client_ip_with_prefix(&candidate, &prefix).is_some() {
            stats.with_ip += 1;
        }

        if let Some(host) = parse_nomad_host_with_prefix(&candidate, meta_role, &prefix) {
            hosts.push(host);
        }
    }

    if hosts.is_empty() {
        return Err(empty_hosts_error(meta_role, &stats, &prefix));
    }

    hosts.sort_by(|a, b| {
        a.client_ip
            .cmp(&b.client_ip)
            .then_with(|| a.name.cmp(&b.name))
    });
    hosts.dedup_by(|a, b| a.client_ip == b.client_ip);
    Ok(hosts)
}

pub async fn available_nomad_host_ips(meta_role: &str) -> Result<Vec<String>, String> {
    Ok(available_nomad_hosts(meta_role)
        .await?
        .into_iter()
        .map(|h| h.client_ip)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_validator_alias_to_validators() {
        assert_eq!(normalize_meta_role("validator"), "validators");
        assert_eq!(normalize_meta_role("validators"), "validators");
        assert_eq!(normalize_meta_role(""), CHAIN_NOMAD_META_ROLE);
    }

    #[test]
    fn reads_role_from_dynamic_meta_only() {
        let node = json!({
            "Status": "ready",
            "Meta": { "role": "builders", "client_ip": "10.0.0.1" },
            "DynamicMeta": { "role": "validators", "client_ip": "192.168.20.5" }
        });
        assert_eq!(node_role(&node).as_deref(), Some("validators"));
        assert_eq!(
            node_client_ip_with_prefix(&node, "192.168.20.").as_deref(),
            Some("192.168.20.5")
        );
        assert!(parse_nomad_host(&node, "validators").is_some());
    }

    #[test]
    fn ignores_static_meta_for_role() {
        let node = json!({
            "Status": "ready",
            "Meta": { "role": "validators", "client_ip": "192.168.20.5" }
        });
        assert_eq!(node_role(&node), None);
        assert!(parse_nomad_host(&node, "validators").is_none());
    }

    #[test]
    fn parses_dynamic_metadata_response() {
        let resp = json!({
            "Meta": { "role": "validators", "foo": "bar" },
            "Dynamic": { "role": "validators", "client_ip": "192.168.20.9", "unset": null }
        });
        let parsed = parse_dynamic_meta_map(&resp);
        assert_eq!(parsed.get("role").and_then(|v| v.as_str()), Some("validators"));
        assert_eq!(
            parsed.get("client_ip").and_then(|v| v.as_str()),
            Some("192.168.20.9")
        );
        assert!(!parsed.contains_key("unset"));
    }

    #[test]
    fn prefers_prefix_from_network_addresses() {
        let node = json!({
            "Status": "ready",
            "DynamicMeta": { "role": "validators" },
            "Attributes": {
                "unique.network.ip-address": "10.0.0.5",
                "unique.network.ip-addresses": "10.0.0.5,192.168.20.42"
            }
        });
        assert_eq!(
            node_client_ip_with_prefix(&node, "192.168.20.").as_deref(),
            Some("192.168.20.42")
        );
    }
}
