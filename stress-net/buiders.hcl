job "stress-test-builder-0" {
  datacenters = ["dc1"]
  type = "batch"

  parameterized {
    meta_required = ["nomad_group"]
    meta_optional = ["node_count", "node_clean", "node_update", "jam_url", "role"]
  }   

  group "stress-builder" {
    count = 2
    constraint {
       attribute = "${meta.role}"
       operator = "="
       value     = "builder"
      }
    constraint {
       distinct_hosts = true
     }
    meta {
      jam_url = "http://192.168.20.0/arkpar/polkajam/stress-test"
      bin_url = "http://192.168.20.0/arkpar/polkajam/target/production"
      jam_id = "stress-test-builder"
      node_update = true
      node_clean = true
      data_dir = "/mnt/nvme_drive_1"
    }

    task "stress-test-task" {
      driver = "raw_exec"
      kill_signal = "SIGKILL"

 
template {
  data = <<EOH
#!/bin/bash
set -x

mkdir -p local
if [ "$NOMAD_META_node_update" = true ]; then
  curl -fsSL -o local/stress-builder "$NOMAD_META_bin_url/stress-builder"
  cp /data/${NOMAD_META_jam_id}/stress-builder local/stress-builder
fi

if [ "$NOMAD_META_node_clean" = true ]; then
  echo "Cleaning location $NOMAD_META_data_dir/$NOMAD_META_jam_id/$INDEX"
  rm -rf "$NOMAD_META_data_dir/$NOMAD_META_jam_id/$INDEX"
fi

curl -fsSL -o local/spec.json "$NOMAD_META_jam_url/spec.json"
chmod +x local/stress-builder

echo "INDEX: $NOMAD_ALLOC_INDEX"
INDEX=$NOMAD_ALLOC_INDEX

START_CORE="0"
if [ $INDEX -eq 1 ]; then
  START_CORE="171"
fi

env
./local/stress-builder --chain=local/spec.json -d $NOMAD_META_data_dir/$NOMAD_META_jam_id/$INDEX --cores=171 --start-core=$START_CORE
sleep 20

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
        RUST_LOG = "jam_node=debug,stress_builder=debug"
      }
   resources {
    cpu = 30000
    memory_max = 32768
    memory = 8192
    }
    

  logs {
    max_files     = 5
    max_file_size = 10
  }
    }
  }
}




