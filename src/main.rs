use anyhow::Result;
use askama::Template;
use axum::{
    extract::{ConnectInfo, Query},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use bollard::{
    container::{ListContainersOptions, StartContainerOptions, StopContainerOptions},
    Docker,
};
use clap::Parser;
use color_eyre::Report;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    fmt,
    fs::read_to_string,
    net::SocketAddr,
    sync::RwLock,
};
use tracing::{instrument, span, Level};
use tracing_subscriber::FmtSubscriber;

lazy_static! {
    static ref CLIENT: Docker = Docker::connect_with_local_defaults().unwrap();
}

lazy_static! {
    static ref COMMON_FILTERS: HashMap<String, Vec<String>> = {
        let mut filters = HashMap::new();
        filters.insert(
            "health".to_string(),
            vec![
                "starting".to_string(),
                "healthy".to_string(),
                "unhealthy".to_string(),
                "none".to_string(),
            ],
        );
        filters.insert(
            "status".to_string(),
            vec![
                "created".to_string(),
                "restarting".to_string(),
                "running".to_string(),
                "removing".to_string(),
                "paused".to_string(),
                "exited".to_string(),
                "dead".to_string(),
            ],
        );
        filters
    };
}

lazy_static! {
    static ref ALLOWED_CONTAINERS: RwLock<Vec<String>> = RwLock::new(Vec::new());
}

#[instrument]
async fn get_allowed_containers() -> Vec<String> {
    let span = span!(Level::INFO, "get_allowed_containers");
    let _guard = span.enter();
    let allowed_containers = ALLOWED_CONTAINERS.read().unwrap().clone();
    tracing::info!("Getting allowed containers");
    allowed_containers
}

#[instrument]
async fn get_container_names() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let span = span!(Level::INFO, "get_container_names");
    let _guard: span::Entered<'_> = span.enter();
    tracing::info!("Getting container names");
    let options = ListContainersOptions {
        all: false,
        filters: COMMON_FILTERS.clone(),
        limit: Some(200),
        size: true,
    };
    let container_list_result = CLIENT.list_containers(Some(options)).await?;
    let container_names: Vec<String> = container_list_result
        .iter()
        .filter_map(|container| container.names.clone())
        .flatten()
        .map(|name| name.trim_start_matches('/').to_string())
        .collect();
    tracing::info!("Fetched container names");
    Ok(container_names)
}

async fn get_container_list_json() -> Json<Vec<Container>> {
    let span = span!(Level::INFO, "get_container_list_json");
    let _guard = span.enter();
    let containers = list_containers().await.unwrap();
    tracing::info!("Container list json: {:?}", &containers);
    Json(containers)
}

async fn get_containers_list_vec() -> Vec<Container> {
    let span = span!(Level::INFO, "get_containers_list_vec");
    let _guard = span.enter();
    let container_info = list_containers().await.unwrap();
    tracing::info!("Container list vec: {:?}", &container_info);
    container_info
}

#[instrument]
async fn init_allowed_containers(args: &Args) {
    let span = span!(Level::INFO, "init_allowed_containers");
    let _guard = span.enter();
    tracing::info!("Initializing allowed containers");
    let containers_from_cli = args
        .containers
        .as_ref()
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok());

    let allowed: Vec<String> = if let Some(containers) = containers_from_cli {
        containers
    } else {
        let containers_from_file =
            load_file_containers(args.file.as_deref().unwrap_or("containers.txt"));
        let containers_from_system = get_container_names().await.unwrap();
        let containers_from_system_set: HashSet<String> =
            containers_from_system.into_iter().collect();
        containers_from_file
            .into_iter()
            .filter(|container| containers_from_system_set.contains(container))
            .collect()
    };

    let mut allowed_containers = ALLOWED_CONTAINERS.write().unwrap();
    tracing::info!("Updated allowed containers");
    *allowed_containers = allowed;
}

#[instrument]
async fn init_logging() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stdout)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

#[instrument]
fn is_container_allowed(container_name: &str) -> bool {
    let span = span!(Level::INFO, "is_container_allowed");
    let _guard = span.enter();
    let allowed_containers = ALLOWED_CONTAINERS.read().unwrap();
    let clean_name = container_name.trim_start_matches('/');
    tracing::info!("Checking if container is allowed");
    allowed_containers.contains(&clean_name.to_string())
}

async fn list_containers() -> Result<Vec<Container>, Box<dyn std::error::Error>> {
    let span = span!(Level::INFO, "list_containers");
    let _guard = span.enter();
    let allowed_names = get_allowed_containers().await;
    let allowed_names_set: HashSet<String> = allowed_names.into_iter().collect();

    let options = ListContainersOptions {
        all: false,
        filters: COMMON_FILTERS.clone(),
        limit: Some(200),
        size: true,
    };

    let container_list_result = CLIENT.list_containers(Some(options)).await?;
    let container_names: Vec<String> = container_list_result
        .iter()
        .filter_map(|container| container.names.clone())
        .flatten()
        .map(|name| name.trim_start_matches('/').to_string())
        .collect();

    tracing::info!("Container names from Docker: {:?}", &container_names);
    let containers: Vec<Container> = container_list_result
        .iter()
        .filter_map(|container| {
            let name = container.names.as_ref().and_then(|names| {
                names
                    .first()
                    .map(|name| name.trim_start_matches('/').to_string())
            });

            let status = container.status.clone();
            let state = container.state.clone();

            if let (Some(name), Some(status), Some(state)) = (name, status, state) {
                if allowed_names_set.contains(&name) {
                    Some(Container {
                        name,
                        status,
                        state,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    Ok(containers)
}

#[instrument]
fn load_file_containers(filename: &str) -> Vec<String> {
    let span = span!(Level::INFO, "load_file_containers");
    let _guard = span.enter();
    tracing::info!("Loading containers from file");
    if std::fs::metadata(filename).is_ok() {
        read_to_string(filename)
            .unwrap()
            .lines()
            .map(|line| line.to_string())
            .collect()
    } else {
        tracing::warn!(?filename, "File does not exist");
        Vec::new()
    }
}

async fn render_containers_html(port: u64) -> Result<Html<String>, Infallible> {
    let span = span!(Level::INFO, "render_containers_html");
    let _guard = span.enter();
    let containers = get_containers_list_vec().await;
    let template = ContainersTemplate { containers, port };
    Ok(Html(template.render().unwrap()))
}

#[derive(Clone, Debug, Serialize, Template)]
#[template(path = "index.html")]
pub struct ContainersTemplate {
    pub containers: Vec<Container>,
    pub port: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Container {
    pub name: String,
    pub status: String,
    pub state: String,
}

impl fmt::Display for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.name, self.status)
    }
}

#[derive(Debug, Deserialize)]
pub struct ContainerName {
    pub name: String,
}

#[tracing::instrument(skip(containername), fields(ip_address))]
async fn start_container_handle(
    Query(containername): Query<ContainerName>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let span = span!(Level::INFO, "start_container_handle");
    let _guard = span.enter();
    tracing::info!("Starting container:{:?}{}", containername, addr);
    if is_container_allowed(&containername.name) {
        let _ = CLIENT
            .start_container(&containername.name, None::<StartContainerOptions<String>>)
            .await;
        (axum::http::StatusCode::OK, "Container started")
    } else {
        tracing::warn!(?containername, "Container not allowed");
        (axum::http::StatusCode::FORBIDDEN, "Container not allowed")
    }
}

#[tracing::instrument(skip(containername), fields(ip_address))]
async fn stop_container_handle(
    Query(containername): Query<ContainerName>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let span = span!(Level::INFO, "stop_container_handle");
    let _guard = span.enter();
    tracing::info!("Stopping container:{:?}{}", containername, addr);
    if is_container_allowed(&containername.name) {
        let _ = CLIENT
            .stop_container(&containername.name, None::<StopContainerOptions>)
            .await;
        (axum::http::StatusCode::OK, "Container stopped")
    } else {
        tracing::warn!(?containername, "Container not allowed");
        (axum::http::StatusCode::FORBIDDEN, "Container not allowed")
    }
}

#[tracing::instrument]
pub fn parse_log_level(log_level: &str) -> Result<Level, anyhow::Error> {
    match log_level.to_lowercase().as_str() {
        "error" => Ok(Level::ERROR),
        "warn" => Ok(Level::WARN),
        "info" => Ok(Level::INFO),
        "debug" => Ok(Level::DEBUG),
        "trace" => Ok(Level::TRACE),
        _ => Ok(Level::INFO),
    }
}

#[tracing::instrument]
pub fn load_logging_config(log_level: Level) -> Result<(), Report> {
    tracing::info!("load_logging_config");
    color_eyre::install()?;
    let subscriber = FmtSubscriber::builder().with_max_level(log_level).finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

/// Simple container management
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Containers specified on the command line (JSON format)
    #[arg(short = 'c', long, value_parser)]
    containers: Option<String>,

    /// Filename to read allowed containers from file
    #[arg(short, long, default_value = "containers.txt")]
    file: Option<String>,

    /// Port number used for server
    #[arg(short, long, default_value_t = 1234)]
    port: u64,

    /// Logging level
    #[clap(short, long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let log_level = parse_log_level(&args.log_level)?;
    let _ = load_logging_config(log_level);

    init_allowed_containers(&args).await;

    let span = span!(Level::INFO, "docker-direct");
    let _guard = span.enter();
    tracing::info!("Starting docker-direct");

    let app = Router::new()
        .route(
            "/containers",
            get(move || render_containers_html(args.port)),
        )
        .route("/containers/start", get(start_container_handle))
        .route("/containers/stop", get(stop_container_handle))
        .route("/containers/statuses", get(get_container_list_json))
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
