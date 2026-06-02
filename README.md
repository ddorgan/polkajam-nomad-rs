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

## CLI (chain / gen-testnet)

Generate a chainspec, validator key seeds, and `spec.json` under `OUTPUT_DIR`
(same flow as `import/lib/gen-testnet.js`). Requires a `polkajam` binary on
`PATH`, or a build at `OUTPUT_DIR/cargo-build/…/polkajam`.

```bash
cd server && cargo build --release
export DEPLOY_CLI="./target/release/deploy-cli"
export APP_DIR=..                    # repo root when running from server/
export OUTPUT_DIR=../output          # default: <APP_DIR>/output
```

### Commands

| Command | What it does |
|---------|----------------|
| `chain list` | List chain IDs under `OUTPUT_DIR` that have a chainspec or spec |
| `chain hosts` | List ready Nomad nodes with dynamic meta `role=validators` (and a host IP) |
| `chain create` | Run `polkajam` keygen + spec build; write `{chainId}_config.json`, `keys/`, zip |

```bash
# List existing chains
deploy-cli chain list

# Preview Nomad hosts (for net_addr when using --use-nomad-hosts)
deploy-cli chain hosts
deploy-cli chain hosts --role validators

# Tiny testnet (6 validators) using an IP range for net_addr
deploy-cli chain create --chain-id mynet --tiny

# Full testnet (1023 validators) with custom IP range
deploy-cli chain create --chain-id stress-test-2 \
  --num-validators 1023 \
  --ip-start 192.168.20.2 \
  --ip-end 192.168.20.83 \
  --base-port 40000

# Use Nomad dynamic metadata instead of an IP range
# Nodes need: nomad node meta apply -node-id <id> role=validators client_ip=...
deploy-cli chain create --chain-id stress-test-2 --tiny --use-nomad-hosts

# Override meta role (default: validators; env NOMAD_CHAIN_META_ROLE also applies)
deploy-cli chain create --chain-id demo --tiny --use-nomad-hosts --nomad-meta-role validators

# JSON output (create prints paths + key count)
deploy-cli --json chain create --chain-id testnet --tiny
```

### Nomad host selection (`--use-nomad-hosts`)

When set, `net_addr` hosts come from ready, eligible Nomad clients:

1. The server calls `GET /v1/client/metadata?node_id=…` and reads the **`Dynamic`**
   metadata map (not static client `Meta` on `/v1/nodes`).
2. Nodes must have dynamic meta `role=validators` (configurable via
   `--nomad-meta-role` or `NOMAD_CHAIN_META_ROLE`).
3. Prefer `client_ip` in dynamic meta; otherwise fall back to node network
   attributes matching `NOMAD_HOST_IP_PREFIX` (default `192.168.20.*`).

Set metadata on a node:

```bash
nomad node meta apply -node-id <full-node-id> role=validators client_ip=192.168.20.5
```

### Output layout

For chain ID `stress-test-2`, files land under `OUTPUT_DIR/stress-test-2/`:

```
stress-test-2_config.json    # chainspec (genesis_validators + net_addr)
spec.json
keys/val_000.seed …
stress-test-2-config-and-keys.zip
polkajam                     # copied if a binary was found
```

Point stress-net `jam_url` at this tree (e.g. `http://host/chains`) so jobs can
fetch spec, config, keys, and the `polkajam` artifact path
`{jam_url}/{jam_id}/polkajam`.

Runnable examples: `examples/chain/`. See `examples/README.md`.

## Endpoints

All endpoints are served by both `app.py` and the Rust `deploy-server`.

- `GET  /`             — the UI (test.hcl)
- `GET  /val7`         — val7.hcl UI
- `GET  /stress`       — stress-net UI (6 / 1023 validators)
- `GET  /chain`        — gen-testnet UI (create chain / chainspec)
- `GET  /api/chains`   — list chains in `OUTPUT_DIR`
- `GET  /api/chains/:id/chainspec` — read `{id}_config.json`
- `GET  /api/nomad/hosts` — ready nodes with dynamic meta `role=validators`
- `POST /api/gen-testnet` — generate chainspec + keys (JSON body, same flags as CLI)
- `GET  /api/options`  — parsed optional meta keys + defaults
- `POST /api/register` — runs `nomad job run test.hcl`
- `POST /api/dispatch` — runs `nomad job dispatch -meta ... multichain-testing`
- `GET  /api/status`   — `nomad status multichain-testing`
- `GET/POST /api/val7/*` — val7 register/dispatch/status
- `GET/POST /api/stress/*` — stress-net register/dispatch/run-target/status
