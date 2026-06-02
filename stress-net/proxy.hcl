job "demo-test-proxy-0" {
  datacenters = ["dc1"]
  type = "batch"

  parameterized {
    meta_optional = ["node_group", "node_count", "node_clean", "node_update", "jam_url", "role"]
  }   

  group "demo-proxy" {
    count = 1 
    constraint {
       attribute = "${meta.role}"
       operator = "="
       value     = "proxy"
      }
    constraint {
       distinct_hosts = true
     }
    meta {
      jam_url = "http://192.168.20.0/chains"
      jam_id = "demo-net"
      node_update = true
      node_clean = true
      data_dir = "/mnt/nvme_drive_1"
    }

    task "demo-test-proxy-task" {
      driver = "raw_exec"
      kill_signal = "SIGKILL"

      artifact {
        source      = "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/polkajam"
        destination = "local/polkajam"
        mode        = "file"
        chown       = true
      }

      template {
  data = <<EOH
#!/bin/bash
set -x
mkdir -p local

mkdir -p "local/${NOMAD_META_jam_id}/keys"
curl -fsSL -o local/spec.json "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/spec.json"
curl -fsSL -o "local/${NOMAD_META_jam_id}_config.json" "${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/${NOMAD_META_jam_id}_config.json"

chmod +x local/polkajam

SEED_URL="${NOMAD_META_jam_url}/${NOMAD_META_jam_id}/keys/proxy.seed"
echo "Seed URL: $SEED_URL"
curl -fsSL -o "local/${NOMAD_META_jam_id}/keys/proxy.seed" "$SEED_URL"



env
./local/polkajam -c local --chain local/spec.json run --data-path data --mode=proxy  --port 5556 --peer-id ekn4mu4jwfwy6ldhq7kpnv5zmgakktkz72kfnlzkse6lqyqmme2ra --external-ip 0.0.0.0
sleep 300

# run the process
EOH
  destination = "local/start.sh"
  perms       = "0755"
}

      config {
        command = "local/start.sh"
      }

      env {
        RUST_BACKTRACE = "full"
        RUST_LOG = "jam_node=debug,demo_proxy=debug"
      }
   resources {
   # cpu = 30000
   # memory_max = 32768
   # memory = 8192
    }
    

  logs {
    max_files     = 5
    max_file_size = 10
   }
  }
 }
}




