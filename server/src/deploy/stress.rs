use crate::deploy::paths::{StressPaths, TARGET_VALIDATORS};
use crate::hcl::{allowed_meta, filter_meta, parse_hcl_file, write_count_variant, ParsedHcl};
use crate::nomad::{
    cmd_result_to_value, cmd_result_with_step, dispatch_cmd, dry_run_step, nomad_addr, run_cmd,
    which_nomad, CmdResult,
};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StressKind {
    Validators,
    Builders,
}

impl StressKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "validators" => Some(Self::Validators),
            "builders" => Some(Self::Builders),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validators => "validators",
            Self::Builders => "builders",
        }
    }
}

pub fn stress_path(paths: &StressPaths, kind: StressKind) -> PathBuf {
    match kind {
        StressKind::Validators => paths.validators.clone(),
        StressKind::Builders => paths.builders.clone(),
    }
}

pub fn stress_options(paths: &StressPaths) -> Value {
    let mut files = Map::new();

    for (label, path) in [
        ("validators", &paths.validators),
        ("builders", &paths.builders),
    ] {
        let mut entry = json!({
            "file": file_name(path),
            "path": path.strip_prefix(&paths.app_dir).unwrap_or(path).to_string_lossy(),
            "exists": path.exists(),
        });
        if path.exists() {
            if let Ok(parsed) = parse_hcl_file(path) {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("optional".into(), json!(parsed.optional));
                    obj.insert("required".into(), json!(parsed.required));
                    obj.insert("defaults".into(), json!(parsed.defaults));
                    obj.insert("job_name".into(), json!(parsed.job_name));
                    obj.insert("count".into(), json!(parsed.count));
                }
            }
        }
        files.insert(label.into(), entry);
    }

    let mut out = json!({
        "target": TARGET_VALIDATORS,
        "nomad_addr": nomad_addr(),
        "nomad_bin": which_nomad(),
        "files": files,
    });

    let per = files
        .get("validators")
        .and_then(|v| v.get("count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    if per > 0 {
        let full = TARGET_VALIDATORS / per;
        let rem = TARGET_VALIDATORS % per;
        if let Some(obj) = out.as_object_mut() {
            obj.insert(
                "plan".into(),
                json!({
                    "target": TARGET_VALIDATORS,
                    "count_per_dispatch": per,
                    "full_dispatches": full,
                    "remainder_count": rem,
                    "total_dispatches": full + if rem > 0 { 1 } else { 0 },
                    "total_validators": full * per + rem,
                }),
            );
        }
    }

    out
}

pub async fn stress_register(paths: &StressPaths, kind: StressKind) -> Result<CmdResult, String> {
    let path = stress_path(paths, kind);
    if !path.exists() {
        return Err(format!("{} not found", file_name(&path)));
    }
    Ok(run_cmd(
        &[
            "nomad".into(),
            "job".into(),
            "run".into(),
            path.to_string_lossy().into_owned(),
        ],
        &paths.app_dir,
    )
    .await)
}

pub async fn stress_dispatch(
    paths: &StressPaths,
    kind: StressKind,
    meta: &Map<String, Value>,
    detach: bool,
) -> Result<CmdResult, String> {
    let path = stress_path(paths, kind);
    if !path.exists() {
        return Err(format!("job spec missing for kind={}", kind.as_str()));
    }
    let parsed = parse_hcl_file(&path).map_err(|e| e.to_string())?;
    let job_name = parsed.job_name.as_deref().unwrap_or("stress-test-1");
    let per = parsed.count.unwrap_or(82);
    let mut filtered = filter_meta(meta, &allowed_meta(&parsed));
    if let Some(group_val) = filtered.get("nomad_group").and_then(|v| v.as_str()) {
        if let Ok(group) = group_val.parse::<u32>() {
            filtered
                .entry("validator_base".to_string())
                .or_insert(json!(((group - 1) * per).to_string()));
            filtered
                .entry("group_size".to_string())
                .or_insert(json!(per.to_string()));
        }
    }
    let args = dispatch_cmd(job_name, &filtered, detach);
    Ok(run_cmd(&args, &paths.app_dir).await)
}

pub struct RunTargetParams<'a> {
    pub target: u32,
    pub meta: &'a Map<String, Value>,
    pub detach: bool,
    pub dry_run: bool,
}

pub struct RunTargetResult {
    pub summary: Value,
    pub failed: bool,
}

pub async fn stress_run_target(
    paths: &StressPaths,
    params: RunTargetParams<'_>,
) -> Result<RunTargetResult, String> {
    if !paths.validators.exists() {
        return Err(format!("{} not found", file_name(&paths.validators)));
    }

    let parsed = parse_hcl_file(&paths.validators).map_err(|e| e.to_string())?;
    let job_name = parsed.job_name.as_deref().unwrap_or("stress-test-1");
    let per = parsed.count.unwrap_or(0);
    if per == 0 {
        return Err("could not determine group count in validators.hcl".into());
    }

    let mut extra_meta = filter_meta(params.meta, &allowed_meta(&parsed));
    extra_meta.remove("nomad_group");

    let full = params.target / per;
    let remainder = params.target % per;
    let remainder_job = format!("{job_name}-remainder");

    let mut steps: Vec<Value> = Vec::new();
    let mut failed = false;

    if params.dry_run {
        steps.push(dry_run_step(
            "register-main",
            &format!("nomad job run {}", paths.validators.display()),
        ));
    } else {
        let result = run_cmd(
            &[
                "nomad".into(),
                "job".into(),
                "run".into(),
                paths.validators.to_string_lossy().into_owned(),
            ],
            &paths.app_dir,
        )
        .await;
        failed |= result.returncode != 0;
        steps.push(cmd_result_with_step("register-main", &result));
    }

    if remainder > 0 {
        write_count_variant(
            &paths.validators,
            &paths.remainder,
            remainder,
            &remainder_job,
        )?;
        if params.dry_run {
            steps.push(dry_run_step(
                "register-remainder",
                &format!("nomad job run {}", paths.remainder.display()),
            ));
        } else {
            let result = run_cmd(
                &[
                    "nomad".into(),
                    "job".into(),
                    "run".into(),
                    paths.remainder.to_string_lossy().into_owned(),
                ],
                &paths.app_dir,
            )
            .await;
            failed |= result.returncode != 0;
            steps.push(cmd_result_with_step("register-remainder", &result));
        }
    }

    let meta_for = |group: u32| -> Map<String, Value> {
        let mut m = extra_meta.clone();
        m.insert("nomad_group".into(), json!(group.to_string()));
        m.insert(
            "validator_base".into(),
            json!(((group - 1) * per).to_string()),
        );
        m.insert("group_size".into(), json!(per.to_string()));
        m
    };

    for i in 1..=full {
        let args = dispatch_cmd(job_name, &meta_for(i), params.detach);
        let step = format!("dispatch-main-{i}");
        if params.dry_run {
            steps.push(dry_run_step(&step, &args.join(" ")));
        } else {
            let result = run_cmd(&args, &paths.app_dir).await;
            failed |= result.returncode != 0;
            steps.push(cmd_result_with_step(&step, &result));
        }
    }

    if remainder > 0 {
        let args = dispatch_cmd(&remainder_job, &meta_for(full + 1), params.detach);
        if params.dry_run {
            steps.push(dry_run_step("dispatch-remainder", &args.join(" ")));
        } else {
            let result = run_cmd(&args, &paths.app_dir).await;
            failed |= result.returncode != 0;
            steps.push(cmd_result_with_step("dispatch-remainder", &result));
        }
    }

    let summary = json!({
        "target": params.target,
        "count_per_dispatch": per,
        "full_dispatches": full,
        "remainder_count": remainder,
        "total_validators": full * per + remainder,
        "main_job": job_name,
        "remainder_job": if remainder > 0 { Value::String(remainder_job) } else { Value::Null },
        "dry_run": params.dry_run,
        "steps": steps,
    });

    Ok(RunTargetResult { summary, failed })
}

pub async fn stress_status(paths: &StressPaths) -> Value {
    let mut jobs = Map::new();

    for (label, path) in [
        ("validators", &paths.validators),
        ("builders", &paths.builders),
        ("remainder", &paths.remainder),
    ] {
        if !path.exists() {
            jobs.insert(label.into(), json!({ "present": false }));
            continue;
        }
        let parsed = match parse_hcl_file(path) {
            Ok(p) => p,
            Err(e) => {
                jobs.insert(
                    label.into(),
                    json!({ "present": true, "error": e.to_string() }),
                );
                continue;
            }
        };
        let Some(name) = parsed.job_name else {
            jobs.insert(label.into(), json!({ "present": true, "error": "no job stanza" }));
            continue;
        };
        let info = run_cmd(
            &["nomad".into(), "status".into(), name.clone()],
            &paths.app_dir,
        )
        .await;
        jobs.insert(
            label.into(),
            json!({
                "present": true,
                "job": name,
                "ok": info.returncode == 0,
                "cmd": info.cmd,
                "returncode": info.returncode,
                "stdout": info.stdout,
                "stderr": info.stderr,
            }),
        );
    }

    json!({ "jobs": jobs })
}

pub fn cmd_result_json(result: &CmdResult) -> Value {
    cmd_result_to_value(result)
}

pub fn default_parsed() -> ParsedHcl {
    ParsedHcl {
        optional: vec![],
        required: vec![],
        defaults: Map::new(),
        job_name: None,
        count: None,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}
