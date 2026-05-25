# stress-net deployment examples

These scripts use `deploy-cli`, the command-line interface for deploying
stress-net Nomad jobs. Build it once from the repo root:

```bash
cd server && cargo build --release
export DEPLOY_CLI="../server/target/release/deploy-cli"
```

Or add the binary to your `PATH`:

```bash
export PATH="$PWD/server/target/release:$PATH"
```

## Prerequisites

- `nomad` on `PATH`
- Nomad cluster reachable via `$NOMAD_ADDR` (and `$NOMAD_TOKEN` if required)
- Run from the repo root, or set `APP_DIR` to the repo root

## Commands

| Script | What it does |
|--------|----------------|
| `options.sh` | Show job specs and the 1023-validator dispatch plan |
| `register-validators.sh` | `nomad job run stress-net/validators.hcl` |
| `register-builders.sh` | `nomad job run stress-net/builders.hcl` |
| `dispatch-group.sh` | Dispatch a single validator group (group 1) |
| `run-target-6-dry-run.sh` | Preview deploying exactly 6 validators |
| `run-target-1023-dry-run.sh` | Preview full stress-net target (1023 validators) |
| `run-target-1023.sh` | Deploy 1023 validators (live) |
| `status.sh` | Check Nomad status for stress-net jobs |

## Direct CLI usage

```bash
# Show options and dispatch plan
deploy-cli stress options

# Register parameterized jobs
deploy-cli stress register --kind validators
deploy-cli stress register --kind builders

# Dispatch one group manually
deploy-cli stress dispatch --kind validators \
  --meta role=validators \
  --meta nomad_group=1

# Deploy exactly N validators (dry-run first)
deploy-cli stress run-target --target 6 --dry-run
deploy-cli stress run-target --target 1023 --dry-run
deploy-cli stress run-target --target 1023 \
  --meta role=validators

# Check job status
deploy-cli stress status

# JSON output for scripting
deploy-cli --json stress options
deploy-cli --json stress run-target --target 6 --dry-run
```

## Meta overrides

Optional dispatch meta keys come from `stress-net/validators.hcl`. Example
overrides are in `meta/stress-dispatch.json`. Pass them on the CLI:

```bash
deploy-cli stress dispatch --kind validators \
  --meta role=validators \
  --meta nomad_group=1 \
  --meta jam_url=http://192.168.20.0/arkpar/polkajam/stress-test \
  --meta node_update=true
```

For `run-target`, do not pass `nomad_group` — the CLI sets group indices
automatically across multiple dispatches.
