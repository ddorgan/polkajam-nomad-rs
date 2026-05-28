use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use serde_json::json;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::paths::resolved_output_dir;
use super::polkajam::{find_polkajam_bin, run_gen_keys, run_gen_spec};

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{stamp}"));
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone)]
pub struct GenTestnetParams {
    pub chain_id: String,
    pub num_validators: Option<u32>,
    pub base_port: u32,
    pub ip_start: String,
    pub ip_end: String,
    pub tiny: bool,
}

impl Default for GenTestnetParams {
    fn default() -> Self {
        Self {
            chain_id: "testnet".into(),
            num_validators: None,
            base_port: 40_000,
            ip_start: "192.168.20.2".into(),
            ip_end: "192.168.20.83".into(),
            tiny: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    pub file_name: String,
    pub seed_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFiles {
    pub chainspec: String,
    pub zip: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenTestnetResult {
    pub chainspec: String,
    pub keys: Vec<KeyEntry>,
    pub output_dir: String,
    pub chain_dir: String,
    pub saved_files: SavedFiles,
}

fn ipv4_to_int(ip: &str) -> Result<u32, String> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return Err(format!("Invalid IPv4 address: {ip}"));
    }
    let mut octets = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        let n: u16 = part
            .parse()
            .map_err(|_| format!("Invalid IPv4 address: {ip}"))?;
        if n > 255 {
            return Err(format!("Invalid IPv4 address: {ip}"));
        }
        octets[i] = n as u8;
    }
    Ok(u32::from_be_bytes(octets))
}

fn int_to_ipv4(n: u32) -> String {
    let bytes = n.to_be_bytes();
    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
}

fn validator_count(params: &GenTestnetParams) -> Result<u32, String> {
    let n = if params.tiny {
        6
    } else {
        params.num_validators.unwrap_or(1023)
    };
    if !(1..=10_000).contains(&n) {
        return Err("numValidators must be between 1 and 10000".into());
    }
    Ok(n)
}

fn collect_seed_files(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_seed_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("seed") {
            let file_name = path
                .file_name()
                .ok_or_else(|| format!("invalid seed path: {}", path.display()))?
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            out.push((file_name, bytes));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn write_zip_entry(
    zip: &mut ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
    name: &str,
    data: &[u8],
) -> Result<(), String> {
    let options = SimpleFileOptions::default();
    zip.start_file(name, options)
        .map_err(|e| e.to_string())?;
    zip.write_all(data).map_err(|e| e.to_string())?;
    Ok(())
}

/// Generate chainspec, validator keys, and spec — same flow as `import/lib/gen-testnet.js`.
pub async fn gen_testnet(app_dir: &Path, params: GenTestnetParams) -> Result<GenTestnetResult, String> {
    let chain_id = params.chain_id.trim();
    if chain_id.is_empty() {
        return Err("chainId must not be empty".into());
    }

    let n = validator_count(&params)?;
    let start_ip = ipv4_to_int(&params.ip_start)?;
    let end_ip = ipv4_to_int(&params.ip_end)?;
    if end_ip < start_ip {
        return Err(format!(
            "IP range invalid: {} > {}",
            params.ip_start, params.ip_end
        ));
    }
    let available = end_ip - start_ip + 1;

    let polkajam_bin = find_polkajam_bin(app_dir)?;

    let tmp_dir = TempDir::new("polkajam-gen")?;
    let tmp_path = tmp_dir.path();

    let mut validators = Vec::with_capacity(n as usize);
    for i in 0..n {
        let file_name = format!("val_{:03}", i);
        let keys = run_gen_keys(&polkajam_bin, tmp_path, &file_name, app_dir).await?;
        let ip = int_to_ipv4(start_ip + (i as u32 % available));
        let net_addr = format!("{}:{}", ip, params.base_port + i);
        validators.push(json!({
            "peer_id": keys.peer_id,
            "bandersnatch": keys.bandersnatch_hex,
            "net_addr": net_addr,
        }));
    }

    let config = json!({
        "id": chain_id,
        "genesis_validators": validators,
    });
    let chainspec = format!("{}\n", serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?);

    let config_path = tmp_path.join(format!("{chain_id}_config.json"));
    std::fs::write(&config_path, &chainspec).map_err(|e| e.to_string())?;

    let spec_path = tmp_path.join("spec.json");
    run_gen_spec(&polkajam_bin, &config_path, &spec_path, app_dir).await?;
    let spec_bytes = std::fs::read(&spec_path).map_err(|e| e.to_string())?;

    let mut seed_files = Vec::new();
    collect_seed_files(tmp_path, &mut seed_files)?;

    let mut keys = Vec::with_capacity(seed_files.len());
    for (file_name, bytes) in &seed_files {
        keys.push(KeyEntry {
            file_name: file_name.clone(),
            seed_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }

    let output_dir = resolved_output_dir(app_dir);
    let chain_dir = output_dir.join(chain_id);
    std::fs::create_dir_all(&chain_dir).map_err(|e| e.to_string())?;

    let chainspec_out_path = chain_dir.join(format!("{chain_id}_config.json"));
    std::fs::write(&chainspec_out_path, &chainspec).map_err(|e| e.to_string())?;

    let mut zip_buffer = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut zip_buffer);
        let mut zip = ZipWriter::new(cursor);
        write_zip_entry(
            &mut zip,
            &format!("{chain_id}_config.json"),
            chainspec.as_bytes(),
        )?;
        write_zip_entry(&mut zip, "spec.json", &spec_bytes)?;
        for (file_name, bytes) in &seed_files {
            write_zip_entry(&mut zip, &format!("keys/{file_name}"), bytes)?;
        }
        if polkajam_bin.is_file() {
            let bin_bytes = std::fs::read(&polkajam_bin).map_err(|e| e.to_string())?;
            write_zip_entry(&mut zip, "polkajam", &bin_bytes)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }

    let zip_out_path = chain_dir.join(format!("{chain_id}-config-and-keys.zip"));
    std::fs::write(&zip_out_path, &zip_buffer).map_err(|e| e.to_string())?;

    let spec_out_path = chain_dir.join("spec.json");
    std::fs::write(&spec_out_path, &spec_bytes).map_err(|e| e.to_string())?;

    let keys_dir = chain_dir.join("keys");
    std::fs::create_dir_all(&keys_dir).map_err(|e| e.to_string())?;
    for (file_name, bytes) in &seed_files {
        std::fs::write(keys_dir.join(file_name), bytes).map_err(|e| e.to_string())?;
    }

    let polkajam_out = chain_dir.join("polkajam");
    if polkajam_bin.is_file() {
        std::fs::copy(&polkajam_bin, &polkajam_out).map_err(|e| e.to_string())?;
        set_executable(&polkajam_out)?;
    }

    Ok(GenTestnetResult {
        chainspec,
        keys,
        output_dir: output_dir.to_string_lossy().into_owned(),
        chain_dir: chain_dir.to_string_lossy().into_owned(),
        saved_files: SavedFiles {
            chainspec: chainspec_out_path.to_string_lossy().into_owned(),
            zip: zip_out_path.to_string_lossy().into_owned(),
        },
    })
}

/// List chain directories under OUTPUT_DIR that contain a chainspec or spec.
pub fn list_chains(app_dir: &Path) -> Result<Vec<String>, String> {
    let output_dir = resolved_output_dir(app_dir);
    if !output_dir.is_dir() {
        return Ok(vec![]);
    }

    let mut chains = Vec::new();
    for entry in std::fs::read_dir(&output_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        let spec_path = dir.join("spec.json");
        let config_path = dir.join(format!("{name}_config.json"));
        if spec_path.is_file() || config_path.is_file() {
            chains.push(name);
        }
    }
    chains.sort();
    Ok(chains)
}
