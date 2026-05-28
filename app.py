from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path

from flask import Flask, jsonify, render_template, request

APP_DIR = Path(__file__).parent.resolve()
HCL_PATH = APP_DIR / "test.hcl"
JOB_NAME = "multichain-testing"

VAL7_HCL = APP_DIR / "val7.hcl"
VAL7_DEFAULT_JOB_NAME = "polkajam-testnet-validators"

STRESS_DIR = APP_DIR / "stress-net"
STRESS_VALIDATORS = STRESS_DIR / "validators.hcl"
STRESS_BUILDERS = STRESS_DIR / "builders.hcl"
STRESS_VALIDATORS_REMAINDER = STRESS_DIR / ".validators-remainder.hcl"

TARGET_VALIDATORS = 1023

app = Flask(__name__)


def parse_hcl(hcl_text: str) -> dict:
    """Best-effort parser for the parameterized meta_optional list and meta {} defaults.

    We intentionally avoid pulling in an HCL2 parser to keep deps tiny; the file
    structure here is simple and stable enough for a regex-based extraction.
    Merges every ``meta { ... }`` block in the file so job-level and group-level
    defaults are both surfaced.
    """
    optional: list[str] = []
    m = re.search(r"meta_optional\s*=\s*\[(.*?)\]", hcl_text, re.DOTALL)
    if m:
        optional = re.findall(r'"([^"]+)"', m.group(1))

    required: list[str] = []
    m = re.search(r"meta_required\s*=\s*\[(.*?)\]", hcl_text, re.DOTALL)
    if m:
        required = re.findall(r'"([^"]+)"', m.group(1))

    count_match = re.search(r"^\s*count\s*=\s*(\d+)", hcl_text, re.MULTILINE)
    count = int(count_match.group(1)) if count_match else None

    defaults: dict[str, object] = {}
    for meta_block in re.finditer(r"meta\s*\{([^{}]*)\}", hcl_text, re.DOTALL):
        body = meta_block.group(1)
        for line in body.splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            kv = re.match(r'([A-Za-z_][\w]*)\s*=\s*(.+)', line)
            if not kv:
                continue
            key, raw = kv.group(1), kv.group(2).strip()
            if raw.startswith('"') and raw.endswith('"'):
                defaults[key] = raw[1:-1]
            elif raw in ("true", "false"):
                defaults[key] = raw == "true"
            else:
                try:
                    defaults[key] = int(raw)
                except ValueError:
                    try:
                        defaults[key] = float(raw)
                    except ValueError:
                        defaults[key] = raw

    job_match = re.search(r'^\s*job\s+"([^"]+)"', hcl_text, re.MULTILINE)
    job_name = job_match.group(1) if job_match else None
    return {
        "optional": optional,
        "required": required,
        "defaults": defaults,
        "job_name": job_name,
        "count": count,
    }


def _dispatch_cmd(job_name: str, meta: dict, detach: bool = True) -> list[str]:
    cmd: list[str] = ["nomad", "job", "dispatch"]
    if detach:
        cmd.append("-detach")
    for key, value in (meta or {}).items():
        if value is None or value == "":
            continue
        cmd.extend(["-meta", f"{key}={value}"])
    cmd.append(job_name)
    return cmd


def _allowed_meta(parsed: dict) -> set[str]:
    """Keys that may be passed to ``nomad job dispatch -meta`` for this job."""
    return set((parsed.get("required") or []) + (parsed.get("optional") or []))


def _filter_meta(meta: dict | None, allowed: set[str]) -> dict:
    """Drop keys that the parameterized job didn't whitelist; nomad would 500."""
    return {k: v for k, v in (meta or {}).items() if k in allowed}


def run_cmd(cmd: list[str], cwd: Path | None = None) -> dict:
    try:
        result = subprocess.run(
            cmd,
            cwd=str(cwd) if cwd else None,
            capture_output=True,
            text=True,
            timeout=120,
            env={**os.environ},
        )
        return {
            "cmd": " ".join(cmd),
            "returncode": result.returncode,
            "stdout": result.stdout,
            "stderr": result.stderr,
        }
    except FileNotFoundError as e:
        return {"cmd": " ".join(cmd), "returncode": 127, "stdout": "", "stderr": str(e)}
    except subprocess.TimeoutExpired as e:
        return {
            "cmd": " ".join(cmd),
            "returncode": 124,
            "stdout": e.stdout or "",
            "stderr": (e.stderr or "") + "\n[timeout]",
        }


@app.get("/")
def index():
    return render_template("index.html", job_name=JOB_NAME, hcl_file=HCL_PATH.name)


@app.get("/api/options")
def api_options():
    if not HCL_PATH.exists():
        return jsonify({"error": f"{HCL_PATH.name} not found"}), 404
    parsed = parse_hcl(HCL_PATH.read_text())
    nomad_addr = os.environ.get("NOMAD_ADDR", "http://127.0.0.1:4646")
    return jsonify(
        {
            "job": JOB_NAME,
            "hcl_file": HCL_PATH.name,
            "nomad_addr": nomad_addr,
            "nomad_bin": shutil.which("nomad"),
            **parsed,
        }
    )


@app.post("/api/register")
def api_register():
    """Run `nomad job run test.hcl` to (re)register the parameterized job."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    if not HCL_PATH.exists():
        return jsonify({"error": f"{HCL_PATH.name} not found"}), 404
    return jsonify(run_cmd(["nomad", "job", "run", str(HCL_PATH)], cwd=APP_DIR))


@app.post("/api/dispatch")
def api_dispatch():
    """Dispatch the parameterized job with user-supplied meta values."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500

    payload = request.get_json(force=True, silent=True) or {}
    meta: dict = payload.get("meta", {})
    detach: bool = bool(payload.get("detach", True))

    allowed = _allowed_meta(parse_hcl(HCL_PATH.read_text())) if HCL_PATH.exists() else set()
    return jsonify(run_cmd(_dispatch_cmd(JOB_NAME, _filter_meta(meta, allowed), detach), cwd=APP_DIR))


@app.get("/api/status")
def api_status():
    if not shutil.which("nomad"):
        return jsonify({"ok": False, "error": "nomad CLI not on PATH"}), 200
    info = run_cmd(["nomad", "status", JOB_NAME])
    return jsonify({"ok": info["returncode"] == 0, **info})


def _val7_parsed() -> dict:
    return parse_hcl(VAL7_HCL.read_text()) if VAL7_HCL.exists() else {}


def _val7_job_name() -> str:
    return _val7_parsed().get("job_name") or VAL7_DEFAULT_JOB_NAME


@app.get("/val7")
def val7_page():
    return render_template("val7.html", job_file=VAL7_HCL.name)


@app.get("/api/val7/options")
def api_val7_options():
    if not VAL7_HCL.exists():
        return jsonify({"error": f"{VAL7_HCL.name} not found"}), 404
    parsed = parse_hcl(VAL7_HCL.read_text())
    return jsonify(
        {
            "job_file": VAL7_HCL.name,
            "job_name": parsed.get("job_name") or VAL7_DEFAULT_JOB_NAME,
            "nomad_addr": os.environ.get("NOMAD_ADDR", "http://127.0.0.1:4646"),
            "nomad_bin": shutil.which("nomad"),
            "optional": parsed.get("optional") or [],
            "defaults": parsed.get("defaults") or {},
        }
    )


@app.post("/api/val7/register")
def api_val7_register():
    """Run `nomad job run val7.hcl` to (re)register the parameterized job."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    if not VAL7_HCL.exists():
        return jsonify({"error": f"{VAL7_HCL.name} not found"}), 404
    return jsonify(run_cmd(["nomad", "job", "run", str(VAL7_HCL)], cwd=APP_DIR))


@app.post("/api/val7/dispatch")
def api_val7_dispatch():
    """Dispatch the val7 parameterized job with user-supplied meta values."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    if not VAL7_HCL.exists():
        return jsonify({"error": f"{VAL7_HCL.name} not found"}), 404

    payload = request.get_json(force=True, silent=True) or {}
    meta: dict = payload.get("meta", {})
    detach: bool = bool(payload.get("detach", True))

    allowed = _allowed_meta(_val7_parsed())
    return jsonify(run_cmd(_dispatch_cmd(_val7_job_name(), _filter_meta(meta, allowed), detach), cwd=APP_DIR))


@app.get("/api/val7/status")
def api_val7_status():
    if not shutil.which("nomad"):
        return jsonify({"ok": False, "error": "nomad CLI not on PATH"}), 200
    name = _val7_job_name()
    info = run_cmd(["nomad", "status", name])
    return jsonify({"ok": info["returncode"] == 0, "job": name, **info})


def _stress_options() -> dict:
    out: dict = {
        "target": TARGET_VALIDATORS,
        "nomad_addr": os.environ.get("NOMAD_ADDR", "http://127.0.0.1:4646"),
        "nomad_bin": shutil.which("nomad"),
        "files": {},
    }
    for label, path in (("validators", STRESS_VALIDATORS), ("builders", STRESS_BUILDERS)):
        entry = {"file": path.name, "path": str(path.relative_to(APP_DIR)), "exists": path.exists()}
        if path.exists():
            entry.update(parse_hcl(path.read_text()))
        out["files"][label] = entry

    validators = out["files"].get("validators") or {}
    per = validators.get("count") or 0
    if per:
        full = TARGET_VALIDATORS // per
        rem = TARGET_VALIDATORS % per
        out["plan"] = {
            "target": TARGET_VALIDATORS,
            "count_per_dispatch": per,
            "full_dispatches": full,
            "remainder_count": rem,
            "total_dispatches": full + (1 if rem else 0),
            "total_validators": full * per + rem,
        }
    return out


def _write_count_variant(src: Path, dst: Path, new_count: int, new_job_name: str) -> None:
    """Copy an HCL job spec to ``dst`` with the group count and job name overridden."""
    text = src.read_text()
    text, n = re.subn(
        r'^(\s*job\s+")[^"]+(")',
        lambda m: f"{m.group(1)}{new_job_name}{m.group(2)}",
        text,
        count=1,
        flags=re.MULTILINE,
    )
    if n == 0:
        raise ValueError(f"could not find job stanza in {src}")
    text, n = re.subn(
        r"^(\s*count\s*=\s*)\d+",
        rf"\g<1>{new_count}",
        text,
        count=1,
        flags=re.MULTILINE,
    )
    if n == 0:
        raise ValueError(f"could not find group count in {src}")
    dst.write_text(text)


def _stress_path_for(kind: str) -> Path | None:
    return {"validators": STRESS_VALIDATORS, "builders": STRESS_BUILDERS}.get(kind)


@app.get("/stress")
def stress_page():
    return render_template(
        "stress.html",
        target=TARGET_VALIDATORS,
        validators_file=STRESS_VALIDATORS.name,
        builders_file=STRESS_BUILDERS.name,
    )


@app.get("/api/stress/options")
def api_stress_options():
    return jsonify(_stress_options())


@app.post("/api/stress/register")
def api_stress_register():
    """Register one of the stress-net jobs (kind: validators | builders)."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    body = request.get_json(force=True, silent=True) or {}
    kind = body.get("kind", "validators")
    path = _stress_path_for(kind)
    if path is None:
        return jsonify({"error": f"unknown kind: {kind}"}), 400
    if not path.exists():
        return jsonify({"error": f"{path.name} not found"}), 404
    return jsonify(run_cmd(["nomad", "job", "run", str(path)], cwd=APP_DIR))


@app.post("/api/stress/dispatch")
def api_stress_dispatch():
    """Single dispatch of a stress-net parameterized job."""
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    body = request.get_json(force=True, silent=True) or {}
    kind = body.get("kind", "validators")
    path = _stress_path_for(kind)
    if path is None or not path.exists():
        return jsonify({"error": f"job spec missing for kind={kind}"}), 404

    parsed = parse_hcl(path.read_text())
    job_name = parsed.get("job_name") or "stress-test-1"
    meta = _filter_meta(body.get("meta"), _allowed_meta(parsed))
    per = parsed.get("count") or 82
    if "nomad_group" in meta:
        try:
            group = int(meta["nomad_group"])
            meta.setdefault("validator_base", str((group - 1) * per))
            meta.setdefault("group_size", str(per))
        except (TypeError, ValueError):
            pass
    detach = bool(body.get("detach", True))
    return jsonify(run_cmd(_dispatch_cmd(job_name, meta, detach), cwd=APP_DIR))


@app.post("/api/stress/run-target")
def api_stress_run_target():
    """Register + dispatch enough times to land on exactly `target` validators.

    With ``count = N`` baked into the validators HCL, a single dispatch creates N
    allocations. To hit a target T that is not a multiple of N, we generate a
    second HCL with ``count = T % N`` and a unique job name, register both, then
    dispatch the main job ``T // N`` times and the remainder job once.
    """
    if not shutil.which("nomad"):
        return jsonify({"error": "nomad CLI not found on PATH"}), 500
    if not STRESS_VALIDATORS.exists():
        return jsonify({"error": f"{STRESS_VALIDATORS.name} not found"}), 404

    body = request.get_json(force=True, silent=True) or {}
    target = int(body.get("target") or TARGET_VALIDATORS)
    extra_meta: dict = body.get("meta") or {}
    detach = bool(body.get("detach", True))
    dry_run = bool(body.get("dry_run", False))

    parsed = parse_hcl(STRESS_VALIDATORS.read_text())
    job_name = parsed.get("job_name") or "stress-test-1"
    per = parsed.get("count") or 0
    if per <= 0:
        return jsonify({"error": "could not determine group count in validators.hcl"}), 500

    allowed = _allowed_meta(parsed)
    extra_meta = _filter_meta(extra_meta, allowed)
    extra_meta.pop("nomad_group", None)

    full = target // per
    remainder = target % per
    remainder_job = job_name

    plan: list[dict] = []
    if not dry_run:
        plan.append(
            {"step": "register-main", **run_cmd(["nomad", "job", "run", str(STRESS_VALIDATORS)], cwd=APP_DIR)}
        )
    else:
        plan.append({"step": "register-main", "cmd": f"nomad job run {STRESS_VALIDATORS}", "dry_run": True})

    if remainder > 0:
        try:
            _write_count_variant(STRESS_VALIDATORS, STRESS_VALIDATORS_REMAINDER, remainder, remainder_job)
        except ValueError as e:
            return jsonify({"error": str(e)}), 500
        if not dry_run:
            plan.append(
                {
                    "step": "register-remainder",
                    **run_cmd(["nomad", "job", "run", str(STRESS_VALIDATORS_REMAINDER)], cwd=APP_DIR),
                }
            )
        else:
            plan.append(
                {
                    "step": "register-remainder",
                    "cmd": f"nomad job run {STRESS_VALIDATORS_REMAINDER}",
                    "dry_run": True,
                }
            )

    def _meta_for(group: int, dispatch_count: int | None = None) -> dict:
        m = dict(extra_meta)
        m["nomad_group"] = str(group)
        m["validator_base"] = str((group - 1) * per)
        m["group_size"] = str(per)
        if dispatch_count is not None:
            m["dispatch_count"] = str(dispatch_count)
        return m

    for i in range(1, full + 1):
        cmd = _dispatch_cmd(job_name, _meta_for(i), detach)
        if dry_run:
            plan.append({"step": f"dispatch-main-{i}", "cmd": " ".join(cmd), "dry_run": True})
        else:
            plan.append({"step": f"dispatch-main-{i}", **run_cmd(cmd, cwd=APP_DIR)})

    if remainder > 0:
        cmd = _dispatch_cmd(remainder_job, _meta_for(full + 1, remainder), detach)
        if dry_run:
            plan.append({"step": "dispatch-remainder", "cmd": " ".join(cmd), "dry_run": True})
        else:
            plan.append({"step": "dispatch-remainder", **run_cmd(cmd, cwd=APP_DIR)})

    summary = {
        "target": target,
        "count_per_dispatch": per,
        "full_dispatches": full,
        "remainder_count": remainder,
        "total_validators": full * per + remainder,
        "main_job": job_name,
        "remainder_job": remainder_job if remainder else None,
        "dry_run": dry_run,
        "steps": plan,
    }
    return jsonify(summary)


@app.get("/api/stress/status")
def api_stress_status():
    if not shutil.which("nomad"):
        return jsonify({"ok": False, "error": "nomad CLI not on PATH"}), 200
    jobs: dict = {}
    for label, path in (
        ("validators", STRESS_VALIDATORS),
        ("builders", STRESS_BUILDERS),
        ("remainder", STRESS_VALIDATORS_REMAINDER),
    ):
        if not path.exists():
            jobs[label] = {"present": False}
            continue
        parsed = parse_hcl(path.read_text())
        name = parsed.get("job_name")
        if not name:
            jobs[label] = {"present": True, "error": "no job stanza"}
            continue
        info = run_cmd(["nomad", "status", name])
        jobs[label] = {
            "present": True,
            "job": name,
            "ok": info["returncode"] == 0,
            **info,
        }
    return jsonify({"jobs": jobs})


if __name__ == "__main__":
    host = os.environ.get("HOST", "127.0.0.1")
    port = int(os.environ.get("PORT", "5050"))
    app.run(host=host, port=port, debug=True)
