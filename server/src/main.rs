use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use deploy_server::deploy::paths::{find_app_dir, resolved_output_dir, StressPaths, TARGET_VALIDATORS};
use deploy_server::deploy::chain::{gen_testnet, list_chains, GenTestnetParams};
use deploy_server::deploy::stress::{
    cmd_result_json, stress_dispatch, stress_options, stress_register, stress_run_target,
    stress_status, RunTargetParams, StressKind,
};
use deploy_server::hcl::{allowed_meta, filter_meta, parse_hcl_file, ParsedHcl};
use deploy_server::nomad::{cmd_result_to_value, dispatch_cmd, nomad_addr, run_cmd, which_nomad};
use deploy_server::nomad_nodes::{available_nomad_hosts, chain_nomad_meta_role, normalize_meta_role};
use minijinja::{context, Environment};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

const JOB_NAME: &str = "multichain-testing";
const VAL7_DEFAULT_JOB_NAME: &str = "polkajam-testnet-validators";

#[derive(Clone)]
struct AppState {
    app_dir: PathBuf,
    templates: Arc<Environment<'static>>,
}

impl AppState {
    fn stress_paths(&self) -> StressPaths {
        StressPaths::new(self.app_dir.clone())
    }

    fn render(&self, name: &str, ctx: minijinja::Value) -> Result<String, minijinja::Error> {
        self.templates.get_template(name)?.render(ctx)
    }
}

#[derive(Debug, Deserialize)]
struct DispatchBody {
    #[serde(default)]
    meta: Map<String, Value>,
    #[serde(default = "default_true")]
    detach: bool,
}

#[derive(Debug, Deserialize)]
struct StressRegisterBody {
    #[serde(default = "default_validators_kind")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct StressDispatchBody {
    #[serde(default = "default_validators_kind")]
    kind: String,
    #[serde(default)]
    meta: Map<String, Value>,
    #[serde(default = "default_true")]
    detach: bool,
}

#[derive(Debug, Deserialize)]
struct RunTargetBody {
    #[serde(default)]
    target: Option<u32>,
    #[serde(default)]
    meta: Map<String, Value>,
    #[serde(default = "default_true")]
    detach: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct GenTestnetBody {
    #[serde(default = "default_chain_id", alias = "chainId")]
    chain_id: String,
    #[serde(default, alias = "numValidators")]
    num_validators: Option<u32>,
    #[serde(default = "default_base_port", alias = "basePort")]
    base_port: u32,
    #[serde(default = "default_ip_start", alias = "ipStart")]
    ip_start: String,
    #[serde(default = "default_ip_end", alias = "ipEnd")]
    ip_end: String,
    #[serde(default)]
    tiny: bool,
    #[serde(default, alias = "useNomadHosts")]
    use_nomad_hosts: bool,
    #[serde(default = "default_nomad_meta_role", alias = "meta.role")]
    nomad_meta_role: String,
}

fn default_nomad_meta_role() -> String {
    chain_nomad_meta_role()
}

fn default_chain_id() -> String {
    "testnet".into()
}

fn default_base_port() -> u32 {
    40_000
}

fn default_ip_start() -> String {
    "192.168.20.2".into()
}

fn default_ip_end() -> String {
    "192.168.20.83".into()
}

fn default_true() -> bool {
    true
}

fn default_validators_kind() -> String {
    "validators".into()
}

fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn nomad_missing() -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "nomad CLI not found on PATH")
}

async fn require_nomad() -> Result<(), Response> {
    if which_nomad().is_none() {
        Err(nomad_missing())
    } else {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "deploy_server=info,tower_http=info".into()),
        )
        .init();

    let app_dir = find_app_dir();
    run_server(app_dir).await;
}

async fn run_server(app_dir: PathBuf) {
    let mut templates = Environment::new();
    templates.set_loader(minijinja::path_loader(app_dir.join("templates")));

    let state = AppState {
        app_dir: app_dir.clone(),
        templates: Arc::new(templates),
    };

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5050);

    let app = Router::new()
        .route("/", get(index_page))
        .route("/val7", get(val7_page))
        .route("/stress", get(stress_page))
        .route("/chain", get(chain_page))
        .route("/api/options", get(api_options))
        .route("/api/register", post(api_register))
        .route("/api/dispatch", post(api_dispatch))
        .route("/api/status", get(api_status))
        .route("/api/val7/options", get(api_val7_options))
        .route("/api/val7/register", post(api_val7_register))
        .route("/api/val7/dispatch", post(api_val7_dispatch))
        .route("/api/val7/status", get(api_val7_status))
        .route("/api/stress/options", get(api_stress_options))
        .route("/api/stress/register", post(api_stress_register))
        .route("/api/stress/dispatch", post(api_stress_dispatch))
        .route("/api/stress/run-target", post(api_stress_run_target))
        .route("/api/stress/status", get(api_stress_status))
        .route("/api/chains", get(api_chains))
        .route("/api/chains/{chain_id}/chainspec", get(api_chainspec))
        .route("/api/gen-testnet", post(api_gen_testnet))
        .route("/api/nomad/hosts", get(api_nomad_hosts))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    tracing::info!("deploy-server listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

async fn index_page(State(state): State<AppState>) -> Result<Html<String>, Response> {
    let hcl = state.app_dir.join("test.hcl");
    let html = state
        .render(
            "index.html",
            context! {
                job_name => JOB_NAME,
                hcl_file => file_name(&hcl),
            },
        )
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Html(html))
}

async fn val7_page(State(state): State<AppState>) -> Result<Html<String>, Response> {
    let val7_hcl = state.app_dir.join("val7.hcl");
    let html = state
        .render(
            "val7.html",
            context! {
                job_file => file_name(&val7_hcl),
            },
        )
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Html(html))
}

async fn stress_page(State(state): State<AppState>) -> Result<Html<String>, Response> {
    let paths = state.stress_paths();
    let html = state
        .render(
            "stress.html",
            context! {
                target => TARGET_VALIDATORS,
                validators_file => file_name(&paths.validators),
                builders_file => file_name(&paths.builders),
            },
        )
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Html(html))
}

async fn chain_page(State(state): State<AppState>) -> Result<Html<String>, Response> {
    let html = state
        .render("chain.html", context! {})
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Html(html))
}

async fn api_options(State(state): State<AppState>) -> Response {
    let hcl = state.app_dir.join("test.hcl");
    if !hcl.exists() {
        return api_error(StatusCode::NOT_FOUND, format!("{} not found", file_name(&hcl)));
    }
    let parsed = match parse_hcl_file(&hcl) {
        Ok(p) => p,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    Json(json!({
        "job": JOB_NAME,
        "hcl_file": file_name(&hcl),
        "nomad_addr": nomad_addr(),
        "nomad_bin": which_nomad(),
        "optional": parsed.optional,
        "required": parsed.required,
        "defaults": parsed.defaults,
        "job_name": parsed.job_name,
        "count": parsed.count,
    }))
    .into_response()
}

async fn api_register(State(state): State<AppState>) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let hcl = state.app_dir.join("test.hcl");
    if !hcl.exists() {
        return api_error(StatusCode::NOT_FOUND, format!("{} not found", file_name(&hcl)));
    }
    let args = vec![
        "nomad".into(),
        "job".into(),
        "run".into(),
        hcl.to_string_lossy().into_owned(),
    ];
    Json(cmd_result_to_value(
        &run_cmd(&args, &state.app_dir).await,
    ))
    .into_response()
}

async fn api_dispatch(State(state): State<AppState>, Json(body): Json<DispatchBody>) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let hcl = state.app_dir.join("test.hcl");
    let allowed = if hcl.exists() {
        allowed_meta(&parse_hcl_file(&hcl).unwrap_or(ParsedHcl {
            optional: vec![],
            required: vec![],
            defaults: Map::new(),
            job_name: None,
            count: None,
        }))
    } else {
        HashSet::new()
    };
    let meta = filter_meta(&body.meta, &allowed);
    let args = dispatch_cmd(JOB_NAME, &meta, body.detach);
    Json(cmd_result_to_value(
        &run_cmd(&args, &state.app_dir).await,
    ))
    .into_response()
}

async fn api_status(State(state): State<AppState>) -> Response {
    if which_nomad().is_none() {
        return Json(json!({ "ok": false, "error": "nomad CLI not on PATH" })).into_response();
    }
    let info = run_cmd(
        &["nomad".into(), "status".into(), JOB_NAME.into()],
        &state.app_dir,
    )
    .await;
    Json(json!({
        "ok": info.returncode == 0,
        "cmd": info.cmd,
        "returncode": info.returncode,
        "stdout": info.stdout,
        "stderr": info.stderr,
    }))
    .into_response()
}

async fn api_val7_options(State(state): State<AppState>) -> Response {
    let val7_hcl = state.app_dir.join("val7.hcl");
    if !val7_hcl.exists() {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("{} not found", file_name(&val7_hcl)),
        );
    }
    let parsed = parse_hcl_file(&val7_hcl).unwrap();
    Json(json!({
        "job_file": file_name(&val7_hcl),
        "job_name": parsed.job_name.as_deref().unwrap_or(VAL7_DEFAULT_JOB_NAME),
        "nomad_addr": nomad_addr(),
        "nomad_bin": which_nomad(),
        "optional": parsed.optional,
        "defaults": parsed.defaults,
    }))
    .into_response()
}

async fn api_val7_register(State(state): State<AppState>) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let val7_hcl = state.app_dir.join("val7.hcl");
    if !val7_hcl.exists() {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("{} not found", file_name(&val7_hcl)),
        );
    }
    let args = vec![
        "nomad".into(),
        "job".into(),
        "run".into(),
        val7_hcl.to_string_lossy().into_owned(),
    ];
    Json(cmd_result_to_value(
        &run_cmd(&args, &state.app_dir).await,
    ))
    .into_response()
}

async fn api_val7_dispatch(
    State(state): State<AppState>,
    Json(body): Json<DispatchBody>,
) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let val7_hcl = state.app_dir.join("val7.hcl");
    if !val7_hcl.exists() {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("{} not found", file_name(&val7_hcl)),
        );
    }
    let parsed = parse_hcl_file(&val7_hcl).unwrap();
    let job_name = parsed
        .job_name
        .as_deref()
        .unwrap_or(VAL7_DEFAULT_JOB_NAME);
    let meta = filter_meta(&body.meta, &allowed_meta(&parsed));
    let args = dispatch_cmd(job_name, &meta, body.detach);
    Json(cmd_result_to_value(
        &run_cmd(&args, &state.app_dir).await,
    ))
    .into_response()
}

async fn api_val7_status(State(state): State<AppState>) -> Response {
    if which_nomad().is_none() {
        return Json(json!({ "ok": false, "error": "nomad CLI not on PATH" })).into_response();
    }
    let val7_hcl = state.app_dir.join("val7.hcl");
    let name = parse_hcl_file(&val7_hcl)
        .ok()
        .and_then(|p| p.job_name)
        .unwrap_or_else(|| VAL7_DEFAULT_JOB_NAME.into());
    let info = run_cmd(
        &["nomad".into(), "status".into(), name.clone()],
        &state.app_dir,
    )
    .await;
    Json(json!({
        "ok": info.returncode == 0,
        "job": name,
        "cmd": info.cmd,
        "returncode": info.returncode,
        "stdout": info.stdout,
        "stderr": info.stderr,
    }))
    .into_response()
}

async fn api_stress_options(State(state): State<AppState>) -> Response {
    Json(stress_options(&state.stress_paths())).into_response()
}

async fn api_stress_register(
    State(state): State<AppState>,
    Json(body): Json<StressRegisterBody>,
) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let kind = match StressKind::parse(&body.kind) {
        Some(k) => k,
        None => return api_error(StatusCode::BAD_REQUEST, format!("unknown kind: {}", body.kind)),
    };
    match stress_register(&state.stress_paths(), kind).await {
        Ok(result) => Json(cmd_result_json(&result)).into_response(),
        Err(e) => api_error(StatusCode::NOT_FOUND, e),
    }
}

async fn api_stress_dispatch(
    State(state): State<AppState>,
    Json(body): Json<StressDispatchBody>,
) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let kind = match StressKind::parse(&body.kind) {
        Some(k) => k,
        None => return api_error(StatusCode::BAD_REQUEST, format!("unknown kind: {}", body.kind)),
    };
    match stress_dispatch(&state.stress_paths(), kind, &body.meta, body.detach).await {
        Ok(result) => Json(cmd_result_json(&result)).into_response(),
        Err(e) => api_error(StatusCode::NOT_FOUND, e),
    }
}

async fn api_stress_run_target(
    State(state): State<AppState>,
    Json(body): Json<RunTargetBody>,
) -> Response {
    if require_nomad().await.is_err() {
        return nomad_missing();
    }
    let target = body.target.unwrap_or(TARGET_VALIDATORS);
    match stress_run_target(
        &state.stress_paths(),
        RunTargetParams {
            target,
            meta: &body.meta,
            detach: body.detach,
            dry_run: body.dry_run,
        },
    )
    .await
    {
        Ok(result) => Json(result.summary).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn api_stress_status(State(state): State<AppState>) -> Response {
    if which_nomad().is_none() {
        return Json(json!({ "ok": false, "error": "nomad CLI not on PATH" })).into_response();
    }
    Json(stress_status(&state.stress_paths()).await).into_response()
}

async fn api_chains(State(state): State<AppState>) -> Response {
    match list_chains(&state.app_dir) {
        Ok(chains) => Json(json!(chains)).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn api_chainspec(
    State(state): State<AppState>,
    AxumPath(chain_id): AxumPath<String>,
) -> Response {
    let chain_id = chain_id.trim();
    if chain_id.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "missing chainId");
    }
    let config_path = resolved_output_dir(&state.app_dir)
        .join(chain_id)
        .join(format!("{chain_id}_config.json"));
    if !config_path.is_file() {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("Chainspec not found for chain {chain_id}"),
        );
    }
    match std::fs::read_to_string(&config_path) {
        Ok(body) => match serde_json::from_str::<Value>(&body) {
            Ok(value) => Json(value).into_response(),
            Err(e) => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Invalid chainspec JSON for chain {chain_id}: {e}"),
            ),
        },
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn api_nomad_hosts() -> Response {
    match available_nomad_hosts(&chain_nomad_meta_role()).await {
        Ok(hosts) => Json(hosts).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn api_gen_testnet(State(state): State<AppState>, Json(body): Json<GenTestnetBody>) -> Response {
    let params = GenTestnetParams {
        chain_id: body.chain_id,
        num_validators: body.num_validators,
        base_port: body.base_port,
        ip_start: body.ip_start,
        ip_end: body.ip_end,
        tiny: body.tiny,
        use_nomad_hosts: body.use_nomad_hosts,
        nomad_meta_role: normalize_meta_role(&body.nomad_meta_role),
    };
    match gen_testnet(&state.app_dir, params).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}
