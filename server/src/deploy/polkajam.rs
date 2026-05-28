use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use super::paths::resolved_output_dir;

#[derive(Debug, Clone)]
pub struct GenKeysOutput {
    pub peer_id: String,
    pub bandersnatch_hex: String,
}

/// Absolute path to a polkajam executable for gen-keys and gen-spec.
/// Resolution order matches `import/lib/polkajam-bin.js`.
pub fn find_polkajam_bin(app_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(bin) = std::env::var("POLKAJAM_BIN") {
        let p = PathBuf::from(bin);
        if !p.is_file() {
            return Err(format!(
                "POLKAJAM_BIN points to a missing file: {}",
                p.display()
            ));
        }
        return Ok(p);
    }

    let default_release = app_dir.join("target/release/polkajam");
    if default_release.is_file() {
        return Ok(default_release);
    }

    let cargo_build = resolved_output_dir(app_dir).join("cargo-build");
    if cargo_build.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&cargo_build) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let candidate = entry.path().join("polkajam");
                    if candidate.is_file() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }

    if let Ok(output) = std::process::Command::new("which")
        .arg("polkajam")
        .output()
    {
        if output.status.success() {
            let line = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !line.is_empty() {
                return Ok(PathBuf::from(line));
            }
        }
    }

    Err(format!(
        "polkajam executable not found. Set POLKAJAM_BIN, build target/release/polkajam under {}, \
         use Binary build to produce OUTPUT_DIR/cargo-build/…/polkajam, or install polkajam on PATH.",
        app_dir.display()
    ))
}

async fn run_polkajam(
    bin: &Path,
    args: &[String],
    cwd: &Path,
    timeout_secs: u64,
) -> Result<(String, String), String> {
    let mut command = Command::new(bin);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(std::env::vars());

    let output = timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .map_err(|_| format!("polkajam timed out after {timeout_secs}s"))?
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(format!(
            "polkajam {} exited {}: {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    Ok((stdout, stderr))
}

pub async fn run_gen_keys(
    bin: &Path,
    out_dir: &Path,
    file_name: &str,
    cwd: &Path,
) -> Result<GenKeysOutput, String> {
    let args = vec![
        format!("-c={}", out_dir.display()),
        "gen-keys".into(),
        file_name.into(),
    ];
    let (stdout, _) = run_polkajam(bin, &args, cwd, 120).await?;

    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    let peer_line = lines.iter().find(|l| l.contains("Peer ID:"));
    let bander_line = lines.iter().find(|l| l.contains("Bandersnatch key:"));
    let (Some(peer_line), Some(bander_line)) = (peer_line, bander_line) else {
        return Err(format!("Failed to parse gen-keys output: {stdout}"));
    };

    let peer_id = peer_line
        .split("Peer ID:")
        .nth(1)
        .ok_or_else(|| format!("Failed to parse peer ID from: {peer_line}"))?
        .trim()
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("Failed to parse peer ID from: {peer_line}"))?
        .to_string();

    let bandersnatch_hex = bander_line
        .split("Bandersnatch key:")
        .nth(1)
        .ok_or_else(|| format!("Failed to parse bandersnatch key from: {bander_line}"))?
        .trim()
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("Failed to parse bandersnatch key from: {bander_line}"))?
        .to_string();

    Ok(GenKeysOutput {
        peer_id,
        bandersnatch_hex,
    })
}

pub async fn run_gen_spec(
    bin: &Path,
    config_path: &Path,
    spec_path: &Path,
    cwd: &Path,
) -> Result<(), String> {
    let args = vec![
        "gen-spec".into(),
        config_path.to_string_lossy().into_owned(),
        spec_path.to_string_lossy().into_owned(),
    ];
    run_polkajam(bin, &args, cwd, 600).await?;
    Ok(())
}
