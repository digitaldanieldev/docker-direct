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
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    fmt,
    fs::File,
    fs::read_to_string,
    net::SocketAddr,
    sync::RwLock,
};
use tracing::{span, Level};
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

async fn init_logging(log_file: &str) {
    let log_file = File::create(log_file).expect("Failed to create log file");
    env_logger::Builder::new()
        .format_timestamp(None)
        .format_module_path(false)
        .filter(Some("docker-direct"), log::LevelFilter::Info)
        .init();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_writer(log_file)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");
}

fn read_file_once(filename: &str) -> Vec<String> {
    let span = span!(Level::INFO, "read_file");
    let _guard = span.enter();
    tracing::info!("Reading file from disk");
    read_to_string(filename)
        .unwrap()
        .lines()
        .map(|line| line.to_string())
        .collect()
}

async fn get_container_names() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let span = span!(Level::INFO, "get_container_names");
    let _guard = span.enter();
    tracing::info!("Getting container names");
    let options = ListContainersOptions {
        all: false,
        filters: COMMON_FILTERS.clone(),
        limit: Some(20),
        size: true,
    };
    let container_list_result = CLIENT.list_containers(Some(options)).await?;
    let container_names: Vec<String> = container_list_result
        .iter()
        .filter_map(|container| container.names.clone())
        .flatten()
        .map(|name| name.trim_start_matches('/').to_string())
        .collect();
    Ok(container_names)
}

async fn initialize_allowed_containers(filename: &str) {
    let containers_from_file = read_file_once(filename);
    let containers_from_system = get_container_names().await.unwrap();
    let containers_from_system_set: HashSet<String> = containers_from_system.into_iter().collect();
    let allowed: Vec<String> = containers_from_file
        .into_iter()
        .filter(|container| containers_from_system_set.contains(container))
        .collect();

    let mut allowed_containers = ALLOWED_CONTAINERS.write().unwrap();
    *allowed_containers = allowed;
}

async fn get_allowed_containers() -> Vec<String> {
    let allowed_containers = ALLOWED_CONTAINERS.read().unwrap().clone();
    allowed_containers
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

#[derive(Clone, Debug, Serialize, Template)]
#[template(path = "index.html")]
pub struct ContainersTemplate {
    pub containers: Vec<Container>,
    pub port: u64,
}

async fn get_containers_info() -> Vec<Container> {
    let container_info = list_containers_info().await.unwrap();
    container_info
}

async fn list_containers_info() -> Result<Vec<Container>, Box<dyn std::error::Error>> {
    let allowed_names = get_allowed_containers().await;
    let allowed_names_set: HashSet<String> = allowed_names.into_iter().collect();

    let options = ListContainersOptions {
        all: false,
        filters: COMMON_FILTERS.clone(),
        limit: Some(20),
        size: true,
    };

    let container_list_result = CLIENT.list_containers(Some(options)).await?;
    let containers: Vec<Container> = container_list_result
        .iter()
        .filter_map(|container| {
            let name = container.names.clone().and_then(|names| {
                names
                    .first()
                    .cloned()
                    .map(|name| name.trim_start_matches('/').to_string())
            });
            let status = container
                .status
                .clone()
                .and_then(|state| Some(state.clone()));
            let state = container
                .state
                .clone()
                .and_then(|state| Some(state.clone()));
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

async fn containers_info_to_html(port: u64) -> Result<Html<String>, Infallible> {
    let containers = get_containers_info().await;
    let template = ContainersTemplate { containers, port };
    Ok(Html(template.render().unwrap()))
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
    tracing::info!("Starting container: {:?}", containername);
    tracing::Span::current().record("ip_address", &tracing::field::display(&addr));

    if is_container_allowed(&containername.name) {
        let _ = CLIENT
            .start_container(&containername.name, None::<StartContainerOptions<String>>)
            .await;
        (axum::http::StatusCode::OK, "Container started")
    } else {
        tracing::warn!(
            "Attempt to start a disallowed container: {:?}",
            containername
        );
        (axum::http::StatusCode::FORBIDDEN, "Container not allowed")
    }
}

#[tracing::instrument(skip(containername), fields(ip_address))]
async fn stop_container_handle(
    Query(containername): Query<ContainerName>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    tracing::info!("Stopping container: {:?}", containername);
    tracing::Span::current().record("ip_address", &tracing::field::display(&addr));

    if is_container_allowed(&containername.name) {
        let _ = CLIENT
            .stop_container(&containername.name, None::<StopContainerOptions>)
            .await;
        (axum::http::StatusCode::OK, "Container stopped")
    } else {
        tracing::warn!(
            "Attempt to stop a disallowed container: {:?}",
            containername
        );
        (axum::http::StatusCode::FORBIDDEN, "Container not allowed")
    }
}

async fn get_container_statuses() -> Json<Vec<Container>> {
    let containers = list_containers_info().await.unwrap();
    Json(containers)
}

fn is_container_allowed(container_name: &str) -> bool {
    let allowed_containers = ALLOWED_CONTAINERS.read().unwrap();
    allowed_containers.contains(&container_name.to_string())
}

/// Simple container management
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Port number used for server
    #[arg(short, long, default_value_t = 1234)]
    port: u64,

    /// Filename to read allowed containers from
    #[arg(short, long, default_value = "containers.txt")]
    allowed: String,

    /// Log file name
    #[arg(short, long, default_value = "docker-direct.log")]
    log: String,
}





#[tokio::main]
async fn main() {
    let args = Args::parse();

    init_logging(&args.log).await;

    initialize_allowed_containers(&args.allowed).await;

    let span = span!(Level::INFO, "docker-direct");
    let _guard = span.enter();
    tracing::info!("Starting docker-direct");

    let app = Router::new()
        .route(
            "/containers",
            get(move || containers_info_to_html(args.port)),
        )
        .route("/containers/start", get(start_container_handle))
        .route("/containers/stop", get(stop_container_handle))
        .route("/containers/statuses", get(get_container_statuses))
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
