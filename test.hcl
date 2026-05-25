job "multichain-testing" {
  datacenters = ["dc1"]
  type = "batch"

  parameterized {

    meta_optional = [ "jam_id", "nomad_group", "node_count", "node_clean", "node_update", "jam_log", "jam_url", "jam_start_ip", "jam_ip_count", "role", "config_root"]
  }

  group "multichain-testing" {
   # lock to one host for testing
   constraint {
      attribute = "${attr.unique.network.ip-address}"
      operator  = "="
      value     = "192.168.20.81"
    }


    meta {
      jam_url = "http://192.168.20.0/chains/multi3"
      jam_id = "multi3"
      config_root = "local"
      jam_log = "info"
      nomad_group = 1
      node_update = true
      node_clean = true
      node_disk_count = 1
      base_port = 40000
      disk_path = "/mnt/nvme_drive_1/nomad/"
      telemetry = "192.168.20.84:9000"
    }

    count = 6

    task "multichain-testing-task" {
      driver = "exec"


template {
  data = <<EOH
#!/bin/bash
set -euo pipefail
set -x


date
hostname

curl -fsSL -o local/${NOMAD_META_jam_id}_config.json "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}_config.json"
export VALIDATOR_INDEX=${NOMAD_ALLOC_INDEX}
PORT="$(( ${NOMAD_META_base_port} + $VALIDATOR_INDEX ))"
export RPC_PORT_OFFSET=400
export DATA_PATH="${NOMAD_META_disk_path}/${NOMAD_NAMESPACE}/${NOMAD_META_jam_id}/${VALIDATOR_INDEX}"

if [ "$NOMAD_META_node_clean" = true ]; then
  echo "Cleaning location ${DATA_PATH}"
  rm -rf ${DATA_PATH} 
fi

env

case "$VALIDATOR_INDEX" in
    0|1)
      echo "=== polkajam validator ${VALIDATOR_INDEX} ==="
      curl -fsSL -o local/polkajam "${NOMAD_META_jam_url}/polkajam"
      curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/spec.json"
      mkdir -p "${DATA_PATH}"

      chmod +x ./local/polkajam
      ./local/polkajam \
        --chain=local/spec.json \
        -c "${NOMAD_META_config_root}" \
        run \
        -d "${DATA_PATH}" \
        --rpc \
        --port "$PORT" \
        --rpc-port $((${NOMAD_META_base_port} + ${VALIDATOR_INDEX} + ${RPC_PORT_OFFSET})) \
        --dev-validator "${VALIDATOR_INDEX}" \
        --external-ip 0.0.0.0 \
        --telemetry "${NOMAD_META_telemetry}"
      ;;
    2|3)
      echo "=== javajam validator ${VALIDATOR_INDEX} ==="
      curl -fsSL -o local/javajam.zip "${NOMAD_META_jam_url}/javajam-linux-x86_64.zip"
      rm -rf local/jam-bundle
      mkdir -p local/jam-bundle
      unzip -qo local/javajam.zip -d local/jam-bundle
      JAVA_BIN=$(find local/jam-bundle -type f -name javajam | head -n1)
      if [ -z "$JAVA_BIN" ]; then
        echo "javajam binary not found in ${NOMAD_META_jam_release}" >&2
        exit 1
      fi
      chmod +x "$JAVA_BIN"
      curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/spec.json"
      "$JAVA_BIN" run \
        --chain ../../local/spec.json \
        -d "${DATA_PATH}" \
        --telemetry "${NOMAD_META_telemetry}" \
        --port "$PORT" \
        --dev-validator "${VALIDATOR_INDEX}"
      ;;
    4|5)
      echo "=== jamduna validator ${VALIDATOR_INDEX} ==="
      curl -fsSL -o local/jamduna "${NOMAD_META_jam_url}/jamduna"

      chmod +x local/jamduna
      ./local/jamduna gen-keys -d "${DATA_PATH}"
      curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/spec.json"

      ./local/jamduna \
        --chain=local/spec.json \
        -c "${NOMAD_META_config_root}" \
        -d "${DATA_PATH}" \
        run \
        --telemetry "${NOMAD_META_telemetry}" \
        --port "$PORT" \
        --dev-validator "${VALIDATOR_INDEX}" \
        --pvm-backend compiler
      ;;
    *)
      echo "triple-binary-validators supports indices 0–5 only (got ${VALIDATOR_INDEX})" >&2
      exit 1
      ;;
  esac
  EOF

EOH
  destination = "local/start.sh"
  perms       = "0755"
}

      config {
        command = "local/start.sh"
      }

      env {
        RUST_BACKTRACE = "full"
        POLKAVM_BACKEND = "compiler"
      }
       resources {
         memory = 20000
        }

  logs {
    max_files     = 5
    max_file_size = 10
  }
    }
  }
}


