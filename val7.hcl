job "polkajam-testnet-validators" {
  datacenters = ["dc1"]
  type        = "batch"
  region      = "global"
  priority    = 50
  namespace   = "default"

  # Parameterized batch job. One dispatch creates `count` validator
  # allocations from this single group definition instead of repeating
  # the task spec per validator. NOMAD_ALLOC_INDEX drives all
  # per-validator differences (port, peer id, data dir).
  parameterized {
    meta_optional = [
      "chain_id",
      "jam_url",
      "base_port",
      "peer_ids",
      "client_ips",
    ]
  }

  meta {
    chain_id   = "testnet7"
    jam_url    = "http://192.168.20.0/chains/testnet7"
    base_port  = "40000"
    peer_ids   = "e2rmtf2rdp2oqqntb7edcjqdko6iw3dzvyyqb4cyz5eczv6e455ga,eljk26fn5rqkjz6rflqxd7uffslj323o5x4ngfrz75zv6xryeldza,eb6zr7xhohdif5reasu3ddv43jh4u77jfgc43xffcbhi3etk5f54b,ekiklz4hj2m7rycoo3r7itp222ifieptl2kzuf5baxm4qaia7vevb,e637qveeq6a7tkz2gc5a4re4ixs6ejcnc4gffgxsyg3rsc3x4dnzb,ekkuy6ofa5mj5eljpryaitltksm2vcgtbqihnqqsvgm3kmywzupaa"
    client_ips = "192.168.20.1,192.168.20.2,192.168.20.3,192.168.20.4,192.168.20.5,192.168.20.6"
  }

  group "validator" {
    count = 6

    # Only schedule on the configured validator hosts (one alloc per host).
    constraint {
      attribute = "${meta.client_ip}"
      operator  = "set_contains_any"
      value     = "192.168.20.1,192.168.20.2,192.168.20.3,192.168.20.4,192.168.20.5,192.168.20.6"
    }

    constraint {
      distinct_hosts = true
    }

    restart {
      attempts = 2
      delay    = "15s"
      interval = "30m"
      mode     = "fail"
    }

    reschedule {
      attempts       = 0
      delay          = "30s"
      delay_function = "exponential"
      max_delay      = "1h"
      unlimited      = true
    }

    ephemeral_disk {
      migrate = false
      size    = 300
      sticky  = false
    }

    volume "data" {
      type      = "host"
      source    = "disk1-local"
      read_only = false
    }

    task "polkajam-validator" {
      driver = "exec"

      config {
        command = "bash"
        args    = ["local/start.sh"]
      }

      volume_mount {
        volume      = "data"
        destination = "/data"
      }

      template {
        destination = "local/start.sh"
        perms       = "0755"
        change_mode = "restart"
        data        = <<EOH
#!/usr/bin/env bash
set -x

VALIDATOR_INDEX=${NOMAD_ALLOC_INDEX}
PEER_FIELD=$((VALIDATOR_INDEX + 1))
PEER_ID=$(printf '%s' "${NOMAD_META_peer_ids}" | cut -d, -f${PEER_FIELD})

mkdir -p "local/${NOMAD_META_chain_id}/keys"

PORT=$((${NOMAD_META_base_port} + VALIDATOR_INDEX))
RPC_PORT=$((PORT + 4000))

SEED_INDEX=$(printf "%03d" "${VALIDATOR_INDEX}")
curl -fsSL -o "local/${NOMAD_META_chain_id}/keys/val.seed" \
  "${NOMAD_META_jam_url}/keys/val_${SEED_INDEX}.seed"
curl -fsSL -o local/polkajam  "${NOMAD_META_jam_url}/polkajam"
curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/spec.json"
chmod +x ./local/polkajam

./local/polkajam --chain=local/spec.json -c local/ run \
  -d "/data/${NOMAD_META_chain_id}/${VALIDATOR_INDEX}" \
  --rpc \
  --rpc-port "${RPC_PORT}" \
  --port "${PORT}" \
  --peer-id "${PEER_ID}" \
  --external-ip 0.0.0.0
EOH
      }

      resources {
        cpu    = 100
        memory = 3192
      }

      logs {
        max_files     = 5
        max_file_size = 10
      }

      kill_timeout = "5s"
    }
  }
}
