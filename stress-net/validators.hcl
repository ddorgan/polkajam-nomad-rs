job "stress-test-2" {
  datacenters = ["dc1"]
  type = "batch"

  parameterized {
    meta_required = ["nomad_group"]
    meta_optional = ["node_count", "node_clean", "node_update", "jam_log", "jam_url", "role", "validator_base", "group_size", "dispatch_count"]
  }   

  group "stress-test-group" {
    count = 82

    constraint {
       attribute = "${meta.role}"
       operator = "="
       value     = "validators"
      }
    constraint {
       distinct_hosts = true
     }
    meta {
      jam_url = "http://192.168.20.0/chains"
      bin_url = "http://192.168.20.0/chains"
      jam_id = "stress-test-2"
      jam_log = "jam_node::rpc=debug,jsonrpsee_server=debug"
      node_update = true
      node_clean = false
      data_dir = "/mnt/nvme_drive_1"
      group_size = "82"
    }

    task "stress-test-task" {
      driver = "raw_exec"
      kill_signal = "SIGKILL"

 
template {
  data = <<EOH
#!/bin/bash
set -euo pipefail
set -x

mkdir -p local

curl -fsSL -o local/polkajam "${NOMAD_META_bin_url}/${NOMAD_META_jam_id}/polkajam"

mkdir -p "local/${NOMAD_META_jam_id}/keys"
curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/spec.json"
curl -fsSL -o "local/${NOMAD_META_jam_id}_config.json" "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/${NOMAD_META_jam_id}_config.json"

chmod +x local/polkajam

export IP="$(ip -4 addr show | awk '/inet 192\.168\.20/ {print $2}' | cut -d/ -f1 | head -n1)"
if [ -z "$IP" ]; then
  echo "Could not detect 192.168.20.x address on this host" >&2
  exit 1
fi

GROUP_SIZE="${NOMAD_META_group_size}"
NOMAD_GROUP="${NOMAD_META_nomad_group}"
if [ -n "${NOMAD_META_validator_base}" ]; then
  VALIDATOR_BASE="${NOMAD_META_validator_base}"
else
  VALIDATOR_BASE="$(( (NOMAD_GROUP - 1) * GROUP_SIZE ))"
fi
if [ -n "${NOMAD_META_dispatch_count}" ]; then
  DISPATCH_COUNT="${NOMAD_META_dispatch_count}"
else
  DISPATCH_COUNT="$GROUP_SIZE"
fi
END="$(( VALIDATOR_BASE + DISPATCH_COUNT - 1 ))"

INDEX=""
for (( idx = VALIDATOR_BASE; idx <= END; idx++ )); do
  entry_ip="$(jq -r --argjson i "$idx" '.genesis_validators[$i].net_addr // empty' "local/${NOMAD_META_jam_id}_config.json" | cut -d: -f1)"
  if [ "$entry_ip" = "$IP" ]; then
    INDEX="$idx"
    break
  fi
done
if [ -z "$INDEX" ]; then
  echo "No validator for host $IP in dispatch range $VALIDATOR_BASE..$END" >&2
  exit 1
fi
VALIDATOR_SEED=$(printf "%03d\n" "$INDEX")

ENTRY="$(jq -r --argjson idx "$INDEX" '.genesis_validators[$idx]' "local/${NOMAD_META_jam_id}_config.json")"
if [ -z "$ENTRY" ] || [ "$ENTRY" = "null" ]; then
  echo "No genesis_validators[$INDEX] in config" >&2
  exit 1
fi

NET_ADDR="$(echo "$ENTRY" | jq -r '.net_addr')"
PEER_ID="$(echo "$ENTRY" | jq -r '.peer_id')"
PORT="$(echo "$NET_ADDR" | cut -d: -f2)"

if [ -z "$NET_ADDR" ] || [ -z "$PEER_ID" ] || [ -z "$PORT" ]; then
  echo "Validator $INDEX missing net_addr or peer_id in config" >&2
  exit 1
fi

export PEER_HOST="$IP:$PORT"

if [ "${NOMAD_META_node_clean}" = "true" ]; then
  echo "Cleaning location ${NOMAD_META_data_dir}/${NOMAD_META_jam_id}/$INDEX"
  rm -rf "${NOMAD_META_data_dir}/${NOMAD_META_jam_id}/$INDEX"
fi

echo "Computed IP: $IP"
echo "Computed INDEX: $INDEX"
echo "Computed VALIDATOR_SEED: $VALIDATOR_SEED"
echo "Computed nomad_group: $NOMAD_GROUP"
echo "Computed validator_base: $VALIDATOR_BASE"
echo "Computed PEER_HOST: $PEER_HOST"
echo "Computed PORT: $PORT"
echo "Computed PEER_ID: $PEER_ID"

SEED_URL="${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/keys/val_$VALIDATOR_SEED.seed"
echo "Seed URL: $SEED_URL"
curl -fsSL -o "local/${NOMAD_META_jam_id}/keys/val.seed" "$SEED_URL"

RPC=""
if [ "$INDEX" -eq 0 ]; then
  RPC="--rpc"
fi

./local/polkajam --chain=local/spec.json -c local run \
  -d "${NOMAD_META_data_dir}/${NOMAD_META_jam_id}/$INDEX" \
  $RPC \
  --rpc-port "$(( PORT + 2000 ))" \
  --port "$PORT" \
  --peer-id "$PEER_ID" \
  --external-ip "$IP"
EOH
  destination = "local/start.sh"
  perms       = "0755"
}

      config {
        command = "local/start.sh"
      }

      env {
        RUST_BACKTRACE = "full"
        RUST_LOG = "peer-misbehaved=debug,authoring=debug,availability=debug,jam_node=debug"
        POLKAVM_BACKEND = "compiler"
      }

  logs {
    max_files     = 5
    max_file_size = 10
  }
    }
  }
}

