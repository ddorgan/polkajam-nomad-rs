use clap::{Parser, Subcommand, ValueEnum};
use deploy_server::deploy::chain::{gen_testnet, list_chains, GenTestnetParams};
use deploy_server::deploy::paths::{find_app_dir, resolved_output_dir, StressPaths, TARGET_VALIDATORS};
use deploy_server::deploy::stress::{
    cmd_result_json, stress_dispatch, stress_options, stress_purge_all, stress_register,
    stress_run_target, stress_status, RunTargetParams, StressKind,
};
use deploy_server::nomad::{nomad_addr, which_nomad};
use deploy_server::nomad_nodes::{available_nomad_hosts, normalize_meta_role, CHAIN_NOMAD_META_ROLE};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "deploy-cli", about = "Deploy stress-net Nomad jobs from the command line")]
struct Cli {
    /// Repository root (defaults to APP_DIR env or auto-detect)
    #[arg(long, global = true)]
    app_dir: Option<PathBuf>,

    /// Emit JSON instead of human-readable output
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Stress-net deployment commands
    Stress {
        #[command(subcommand)]
        command: StressCommands,
    },
    /// Chain generation (gen-testnet)
    Chain {
        #[command(subcommand)]
        command: ChainCommands,
    },
}

#[derive(Subcommand)]
enum StressCommands {
    /// Show parsed job specs, defaults, and dispatch plan for the default target
    Options,
    /// Register a parameterized job (`nomad job run`)
    Register {
        #[arg(long, value_enum, default_value_t = StressKindArg::Validators)]
        kind: StressKindArg,
    },
    /// Dispatch one group (`nomad job dispatch`)
    Dispatch {
        #[arg(long, value_enum, default_value_t = StressKindArg::Validators)]
        kind: StressKindArg,
        /// Meta key=value (repeatable). Example: --meta role=validators --meta nomad_group=1
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
        /// Wait for dispatch to finish instead of passing -detach
        #[arg(long)]
        no_detach: bool,
    },
    /// Register + dispatch until exactly `--target` validators are running
    RunTarget {
        #[arg(long, default_value_t = TARGET_VALIDATORS)]
        target: u32,
        #[arg(long = "meta", value_name = "KEY=VALUE")]
        meta: Vec<String>,
        /// Print planned commands without running nomad
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_detach: bool,
    },
    /// `nomad status` for validators, builders, and remainder jobs
    Status,
    /// Stop and purge stress-net dispatch instances and parent jobs (`nomad job stop -purge`)
    Purge {
        /// Print planned commands without running nomad
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ChainCommands {
    /// Generate chainspec, validator keys, and spec under OUTPUT_DIR
    Create {
        #[arg(long, default_value = "testnet")]
        chain_id: String,
        #[arg(long)]
        num_validators: Option<u32>,
        #[arg(long, default_value_t = 40_000)]
        base_port: u32,
        #[arg(long, default_value = "192.168.20.2")]
        ip_start: String,
        #[arg(long, default_value = "192.168.20.83")]
        ip_end: String,
        /// Generate 6 validators instead of `--num-validators`
        #[arg(long)]
        tiny: bool,
        /// Use ready Nomad nodes (`/v1/client/metadata` dynamic meta role=validators) for net_addr IPs
        #[arg(long)]
        use_nomad_hosts: bool,
        /// Dynamic meta role to match (default: validators)
        #[arg(long, default_value = CHAIN_NOMAD_META_ROLE)]
        nomad_meta_role: String,
    },
    /// List ready Nomad hosts by dynamic metadata role (default: validators)
    Hosts {
        #[arg(long, default_value = CHAIN_NOMAD_META_ROLE)]
        role: String,
    },
    /// List chains in OUTPUT_DIR
    List,
}

#[derive(Clone, Copy, ValueEnum)]
enum StressKindArg {
    Validators,
    Builders,
}

impl From<StressKindArg> for StressKind {
    fn from(v: StressKindArg) -> Self {
        match v {
            StressKindArg::Validators => StressKind::Validators,
            StressKindArg::Builders => StressKind::Builders,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let app_dir = cli.app_dir.clone().unwrap_or_else(find_app_dir);
    let paths = StressPaths::new(app_dir.clone());

    match cli.command {
        Commands::Stress { ref command } => match command {
            StressCommands::Options => {
                let out = stress_options(&paths);
                if cli.json {
                    print_json(&out);
                } else {
                    print_stress_options(&out);
                }
                ExitCode::SUCCESS
            }
            StressCommands::Register { kind } => {
                if let Err(e) = require_nomad(&cli) {
                    return e;
                }
                match stress_register(&paths, (*kind).into()).await {
                    Ok(result) => exit_from_cmd(&cli, &result),
                    Err(e) => exit_error(&cli, e),
                }
            }
            StressCommands::Dispatch {
                kind,
                meta,
                no_detach,
            } => {
                if let Err(e) = require_nomad(&cli) {
                    return e;
                }
                let meta = match parse_meta_pairs(&meta) {
                    Ok(m) => m,
                    Err(e) => return exit_error(&cli, e),
                };
                match stress_dispatch(&paths, (*kind).into(), &meta, !*no_detach).await {
                    Ok(result) => exit_from_cmd(&cli, &result),
                    Err(e) => exit_error(&cli, e),
                }
            }
            StressCommands::RunTarget {
                target,
                meta,
                dry_run,
                no_detach,
            } => {
                if let Err(e) = require_nomad(&cli) {
                    return e;
                }
                let meta = match parse_meta_pairs(&meta) {
                    Ok(m) => m,
                    Err(e) => return exit_error(&cli, e),
                };
                match stress_run_target(
                    &paths,
                    RunTargetParams {
                        target: *target,
                        meta: &meta,
                        detach: !*no_detach,
                        dry_run: *dry_run,
                    },
                )
                .await
                {
                    Ok(result) => {
                        if cli.json {
                            print_json(&result.summary);
                        } else {
                            print_run_target(&result.summary);
                        }
                        if result.failed && !*dry_run {
                            ExitCode::from(1)
                        } else {
                            ExitCode::SUCCESS
                        }
                    }
                    Err(e) => exit_error(&cli, e),
                }
            }
            StressCommands::Status => {
                let out = if which_nomad().is_none() {
                    json!({ "ok": false, "error": "nomad CLI not on PATH" })
                } else {
                    stress_status(&paths).await
                };
                if cli.json {
                    print_json(&out);
                } else {
                    print_stress_status(&out);
                }
                ExitCode::SUCCESS
            }
            StressCommands::Purge { dry_run } => {
                if let Err(e) = require_nomad(&cli) {
                    return e;
                }
                match stress_purge_all(&paths, *dry_run).await {
                    Ok(result) => {
                        if cli.json {
                            print_json(&result.summary);
                        } else {
                            print_purge_all(&result.summary);
                        }
                        if result.failed && !*dry_run {
                            ExitCode::from(1)
                        } else {
                            ExitCode::SUCCESS
                        }
                    }
                    Err(e) => exit_error(&cli, e),
                }
            }
        },
        Commands::Chain { ref command } => match command {
            ChainCommands::Create {
                chain_id,
                num_validators,
                base_port,
                ip_start,
                ip_end,
                tiny,
                use_nomad_hosts,
                nomad_meta_role,
            } => {
                let params = GenTestnetParams {
                    chain_id: chain_id.clone(),
                    num_validators: *num_validators,
                    base_port: *base_port,
                    ip_start: ip_start.clone(),
                    ip_end: ip_end.clone(),
                    tiny: *tiny,
                    use_nomad_hosts: *use_nomad_hosts,
                    nomad_meta_role: normalize_meta_role(nomad_meta_role),
                };
                match gen_testnet(&app_dir, params).await {
                    Ok(result) => {
                        if cli.json {
                            print_json(&serde_json::to_value(&result).unwrap());
                        } else {
                            print_chain_create(&result);
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => exit_error(&cli, e),
                }
            }
            ChainCommands::Hosts { role } => {
                let result = available_nomad_hosts(role).await;
                match result {
                    Ok(hosts) => {
                        if cli.json {
                            print_json(&serde_json::to_value(&hosts).unwrap());
                        } else {
                            println!("nomad hosts ({}):", hosts.len());
                            for h in hosts {
                                println!(
                                    "  {}\t{}\t{}\t{}",
                                    h.client_ip, h.name, h.status, h.datacenter
                                );
                            }
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => exit_error(&cli, e),
                }
            }
            ChainCommands::List => match list_chains(&app_dir) {
                Ok(chains) => {
                    if cli.json {
                        print_json(&json!(chains));
                    } else {
                        println!("chains in {}", resolved_output_dir(&app_dir).display());
                        if chains.is_empty() {
                            println!("  (none)");
                        } else {
                            for chain in chains {
                                println!("  {chain}");
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => exit_error(&cli, e),
            },
        },
    }
}

fn require_nomad(cli: &Cli) -> Result<(), ExitCode> {
    if which_nomad().is_none() {
        Err(exit_error(cli, "nomad CLI not found on PATH".to_string()))
    } else {
        Ok(())
    }
}

fn parse_meta_pairs(pairs: &[String]) -> Result<Map<String, Value>, String> {
    let mut meta = Map::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("invalid meta (expected KEY=VALUE): {pair}"));
        };
        if key.is_empty() {
            return Err(format!("invalid meta key in: {pair}"));
        }
        meta.insert(key.to_string(), Value::String(value.to_string()));
    }
    Ok(meta)
}

fn exit_from_cmd(cli: &Cli, result: &deploy_server::nomad::CmdResult) -> ExitCode {
    if cli.json {
        print_json(&cmd_result_json(result));
    } else {
        print_cmd_result(result);
    }
    if result.returncode == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn exit_error(cli: &Cli, message: String) -> ExitCode {
    if cli.json {
        print_json(&json!({ "error": message }));
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::from(1)
}

fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn print_cmd_result(result: &deploy_server::nomad::CmdResult) {
    println!("$ {}", result.cmd);
    if !result.stdout.is_empty() {
        print!("{}", result.stdout);
        if !result.stdout.ends_with('\n') {
            println!();
        }
    }
    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
        if !result.stderr.ends_with('\n') {
            eprintln!();
        }
    }
    if result.returncode != 0 {
        eprintln!("exit code: {}", result.returncode);
    }
}

fn print_stress_options(out: &Value) {
    println!("stress-net options");
    println!("  app target:   {}", out["target"]);
    println!("  nomad addr:   {}", nomad_addr());
    println!("  nomad bin:    {}", out["nomad_bin"].as_str().unwrap_or("(not found)"));

    if let Some(plan) = out.get("plan") {
        println!();
        println!("plan for {} validators:", plan["target"]);
        println!("  per dispatch: {}", plan["count_per_dispatch"]);
        println!("  full groups:  {}", plan["full_dispatches"]);
        println!("  remainder:    {}", plan["remainder_count"]);
        println!("  total:        {}", plan["total_validators"]);
    }

    if let Some(files) = out.get("files").and_then(|v| v.as_object()) {
        println!();
        for (label, entry) in files {
            println!("{label}:");
            println!("  file: {}", entry["file"]);
            println!("  exists: {}", entry["exists"]);
            if let Some(name) = entry.get("job_name").and_then(|v| v.as_str()) {
                println!("  job: {}", name);
            }
            if let Some(count) = entry.get("count") {
                println!("  count: {}", count);
            }
        }
    }
}

fn print_run_target(summary: &Value) {
    println!("stress-net run-target");
    println!("  target:           {}", summary["target"]);
    println!("  per dispatch:     {}", summary["count_per_dispatch"]);
    println!("  full dispatches:  {}", summary["full_dispatches"]);
    println!("  remainder:        {}", summary["remainder_count"]);
    println!("  total validators: {}", summary["total_validators"]);
    if summary["dry_run"].as_bool() == Some(true) {
        println!("  mode:             dry-run");
    }
    println!();
    if let Some(steps) = summary.get("steps").and_then(|v| v.as_array()) {
        for step in steps {
            let name = step["step"].as_str().unwrap_or("step");
            if step.get("dry_run").and_then(|v| v.as_bool()) == Some(true) {
                println!("[{name}] $ {}", step["cmd"].as_str().unwrap_or(""));
            } else {
                let rc = step["returncode"].as_i64().unwrap_or(-1);
                println!("[{name}] exit {rc}");
                if let Some(cmd) = step.get("cmd").and_then(|v| v.as_str()) {
                    println!("  $ {cmd}");
                }
                if let Some(stderr) = step.get("stderr").and_then(|v| v.as_str()) {
                    if !stderr.is_empty() {
                        eprintln!("  {stderr}");
                    }
                }
            }
        }
    }
}

fn print_purge_all(summary: &Value) {
    println!("stress-net purge");
    if let Some(jobs) = summary.get("dispatch_jobs").and_then(|v| v.as_array()) {
        println!("  dispatch jobs: {}", jobs.len());
        for job in jobs.iter().filter_map(|v| v.as_str()) {
            println!("    {job}");
        }
    }
    if let Some(jobs) = summary.get("parents").and_then(|v| v.as_array()) {
        println!(
            "  parent jobs: {}",
            jobs.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    if summary["dry_run"].as_bool() == Some(true) {
        println!("  mode: dry-run");
    }
    println!();
    if let Some(steps) = summary.get("steps").and_then(|v| v.as_array()) {
        for step in steps {
            let name = step["step"].as_str().unwrap_or("step");
            if step.get("dry_run").and_then(|v| v.as_bool()) == Some(true) {
                println!("[{name}] $ {}", step["cmd"].as_str().unwrap_or(""));
            } else {
                let rc = step["returncode"].as_i64().unwrap_or(-1);
                println!("[{name}] exit {rc}");
                if let Some(cmd) = step.get("cmd").and_then(|v| v.as_str()) {
                    println!("  $ {cmd}");
                }
                if let Some(stderr) = step.get("stderr").and_then(|v| v.as_str()) {
                    if !stderr.is_empty() {
                        eprintln!("  {stderr}");
                    }
                }
            }
        }
    }
}

fn print_stress_status(out: &Value) {
    if out.get("ok") == Some(&Value::Bool(false)) {
        println!("{}", out["error"].as_str().unwrap_or("nomad unavailable"));
        return;
    }
    println!("stress-net status");
    if let Some(jobs) = out.get("jobs").and_then(|v| v.as_object()) {
        for (label, entry) in jobs {
            print!("  {label}: ");
            if entry.get("present") == Some(&Value::Bool(false)) {
                println!("(spec missing)");
                continue;
            }
            if let Some(err) = entry.get("error").and_then(|v| v.as_str()) {
                println!("error: {err}");
                continue;
            }
            let job = entry.get("job").and_then(|v| v.as_str()).unwrap_or("?");
            let ok = entry.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            println!("{job} ({})", if ok { "ok" } else { "not found / error" });
            if let Some(stdout) = entry.get("stdout").and_then(|v| v.as_str()) {
                if !stdout.trim().is_empty() {
                    for line in stdout.lines().take(8) {
                        println!("    {line}");
                    }
                }
            }
        }
    }
}

fn print_chain_create(result: &deploy_server::deploy::GenTestnetResult) {
    println!("chain created");
    println!("  output dir:  {}", result.output_dir);
    println!("  chain dir:   {}", result.chain_dir);
    println!("  chainspec:   {}", result.saved_files.chainspec);
    println!("  zip:         {}", result.saved_files.zip);
    println!("  key seeds:   {}", result.keys.len());
    if let Some(hosts) = &result.nomad_hosts {
        println!(
            "  nomad hosts: {} (dynamic meta role={})",
            hosts.len(),
            hosts.first().map(|h| h.role.as_str()).unwrap_or(CHAIN_NOMAD_META_ROLE)
        );
    }
}
