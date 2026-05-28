use serde_json::{json, Map, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CmdResult {
    pub cmd: String,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn which_nomad() -> Option<String> {
    std::process::Command::new("which")
        .arg("nomad")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn nomad_addr() -> String {
    std::env::var("NOMAD_ADDR").unwrap_or_else(|_| "http://127.0.0.1:4646".to_string())
}

pub fn dispatch_cmd(job_name: &str, meta: &Map<String, Value>, detach: bool) -> Vec<String> {
    let mut cmd = vec!["nomad".into(), "job".into(), "dispatch".into()];
    if detach {
        cmd.push("-detach".into());
    }
    for (key, value) in meta {
        if value_is_empty(value) {
            continue;
        }
        cmd.push("-meta".into());
        cmd.push(format!("{key}={}", meta_value_as_str(value)));
    }
    cmd.push(job_name.into());
    cmd
}

pub async fn run_cmd(args: &[String], cwd: &Path) -> CmdResult {
    let cmd_line = args.join(" ");
    let program = args.first().map(String::as_str).unwrap_or("nomad");

    let mut command = Command::new(program);
    if args.len() > 1 {
        command.args(&args[1..]);
    }
    command
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(std::env::vars());

    match timeout(Duration::from_secs(120), command.output()).await {
        Ok(Ok(output)) => CmdResult {
            cmd: cmd_line,
            returncode: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Ok(Err(err)) => CmdResult {
            cmd: cmd_line,
            returncode: if err.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                1
            },
            stdout: String::new(),
            stderr: err.to_string(),
        },
        Err(_) => CmdResult {
            cmd: cmd_line,
            returncode: 124,
            stdout: String::new(),
            stderr: "[timeout]".into(),
        },
    }
}

pub fn cmd_result_to_value(result: &CmdResult) -> Value {
    json!({
        "cmd": result.cmd,
        "returncode": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
    })
}

pub fn cmd_result_with_step(step: &str, result: &CmdResult) -> Value {
    let mut v = cmd_result_to_value(result);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("step".into(), Value::String(step.into()));
    }
    v
}

/// Job IDs from `nomad job status` (first column of the table).
pub fn parse_job_status_ids(stdout: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("ID") || line.starts_with("==>") {
            continue;
        }
        if let Some(id) = line.split_whitespace().next() {
            if !id.is_empty() {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

pub fn purge_job_args(job_id: &str) -> Vec<String> {
    vec![
        "nomad".into(),
        "job".into(),
        "stop".into(),
        "-purge".into(),
        "-yes".into(),
        "-detach".into(),
        job_id.to_string(),
    ]
}

pub fn dry_run_step(step: &str, cmd: &str) -> Value {
    json!({
        "step": step,
        "cmd": cmd,
        "dry_run": true,
    })
}

fn meta_value_as_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}
