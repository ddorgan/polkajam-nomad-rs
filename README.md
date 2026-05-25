# polkajam-specific-deploy

A tiny web UI to dispatch the parameterized Nomad job defined in `test.hcl`
(`multichain-testing`) with custom meta values.

## What it does

`test.hcl` is a Nomad **parameterized batch job**. The UI reads its
`parameterized.meta_optional` list and the defaults in its `meta {}` block,
renders a form, and on submit it shells out to the local `nomad` CLI:

```
nomad job run test.hcl                       # Register job
nomad job dispatch -meta k=v ... multichain-testing   # Dispatch
```

## Requirements

- Python 3.9+
- The `nomad` CLI on `PATH` (verified at `~/homebrew/bin/nomad` on this box)
- A reachable Nomad server (`$NOMAD_ADDR`, defaults to `http://127.0.0.1:4646`)

## Run (Python)

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
python3 app.py                 # http://127.0.0.1:5050
```

## Run (Rust)

```bash
cd server
cargo run --release            # http://127.0.0.1:5050
```

The Rust server is a drop-in replacement for `app.py`: same routes, templates, and
Nomad CLI integration. Set `APP_DIR` to the repo root if you run the binary from
elsewhere (defaults to the parent directory when started from `server/`).

Optional env vars: `HOST`, `PORT`, `APP_DIR`, plus the usual Nomad ones
(`NOMAD_ADDR`, `NOMAD_TOKEN`, `NOMAD_NAMESPACE`).

## CLI (stress-net)

Build the CLI from `server/`:

```bash
cd server && cargo build --release
./target/release/deploy-cli stress options
```

Commands mirror the `/api/stress/*` web endpoints:

```bash
deploy-cli stress options                              # job specs + 1023 plan
deploy-cli stress register --kind validators           # nomad job run
deploy-cli stress register --kind builders
deploy-cli stress dispatch --kind validators --meta role=validators --meta nomad_group=1
deploy-cli stress run-target --target 1023 --dry-run   # preview full deploy
deploy-cli stress run-target --target 1023             # live deploy
deploy-cli stress status
```

Use `--json` for machine-readable output. Set `APP_DIR` to the repo root if
you run the binary from elsewhere. See `examples/` for runnable shell scripts.

## Endpoints

All endpoints are served by both `app.py` and the Rust `deploy-server`.

- `GET  /`             — the UI (test.hcl)
- `GET  /val7`         — val7.hcl UI
- `GET  /stress`       — stress-net UI (6 / 1023 validators)
- `GET  /api/options`  — parsed optional meta keys + defaults
- `POST /api/register` — runs `nomad job run test.hcl`
- `POST /api/dispatch` — runs `nomad job dispatch -meta ... multichain-testing`
- `GET  /api/status`   — `nomad status multichain-testing`
- `GET/POST /api/val7/*` — val7 register/dispatch/status
- `GET/POST /api/stress/*` — stress-net register/dispatch/run-target/status
