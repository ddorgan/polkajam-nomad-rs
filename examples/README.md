# deploy-cli examples

These scripts use `deploy-cli` from `server/`. Build once from the repo root:

```bash
cd server && cargo build --release
export DEPLOY_CLI="../server/target/release/deploy-cli"
```

Or add the binary to your `PATH`:

```bash
export PATH="$PWD/server/target/release:$PATH"
```

## Prerequisites

- Run from the repo root, or set `APP_DIR` to the repo root
- **stress-net**: `nomad` on `PATH`, cluster reachable via `$NOMAD_ADDR` (and `$NOMAD_TOKEN` if required)
- **chain**: `polkajam` on `PATH` (or under `OUTPUT_DIR` from a cargo build); for `--use-nomad-hosts`, Nomad API access as above

## Chain (gen-testnet)

| Script | What it does |
|--------|----------------|
| `chain/list.sh` | `deploy-cli chain list` |
| `chain/hosts.sh` | `deploy-cli chain hosts` (Nomad nodes with dynamic `role=validators`) |
| `chain/create-tiny.sh` | 6 validators, IP range (`CHAIN_ID` env, default `testnet`) |
| `chain/create-tiny-nomad-hosts.sh` | 6 validators from Nomad host IPs |
| `chain/create-stress-test-2.sh` | 1023 validators for `stress-test-2` |

```bash
chmod +x examples/chain/*.sh
./examples/chain/list.sh
./examples/chain/hosts.sh
CHAIN_ID=mynet ./examples/chain/create-tiny.sh
./examples/chain/create-tiny-nomad-hosts.sh
./examples/chain/create-stress-test-2.sh
```

See the main [README](../README.md#cli-chain--gen-testnet) for full `chain` CLI flags and Nomad metadata notes.

## Stress-net commands

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
