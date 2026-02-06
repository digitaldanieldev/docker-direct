use anyhow::Result;
use askama::Template;
use axum::{
    extract::{ConnectInfo, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use bollard::{
    container::{
        InspectContainerOptions, ListContainersOptions, StartContainerOptions,
        StatsOptions, StopContainerOptions,
    },
    Docker,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::read_to_string,
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

struct AppState {
    docker: Docker,
    allowed_containers: Vec<String>,
    port: u64,
    container_cache: RwLock<Vec<ContainerInfo>>,
    cache_generation: std::sync::atomic::AtomicU64,
    common_filters: HashMap<String, Vec<String>>,
    mc_cache: RwLock<HashMap<String, MinecraftInfo>>,
}

type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModEntry {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayerInfo {
    pub name: String,
    #[serde(skip)]
    pub x: Option<f64>,
    #[serde(skip)]
    pub y: Option<f64>,
    #[serde(skip)]
    pub z: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MinecraftInfo {
    pub version: String,
    pub player_count: i32,
    pub max_players: i32,
    pub players: Vec<PlayerInfo>,
    pub mods: Vec<ModEntry>,
    pub seed: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    pub state: String,
    pub status: String,
    pub image: String,
    pub created: String,
    pub uptime: String,
    pub ports: Vec<PortMapping>,
    pub cpu_percent: f64,
    pub memory_usage: u64,
    pub memory_limit: u64,
    pub restart_count: i64,
    pub minecraft: Option<MinecraftInfo>,
    #[serde(skip)]
    pub rcon_password: String,
}

// Keep the old Container struct for the template (initial HTML render)
#[derive(Clone, Debug, Serialize)]
pub struct Container {
    pub name: String,
    pub status: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Template)]
#[template(path = "index.html")]
pub struct ContainersTemplate {
    pub containers: Vec<Container>,
    pub port: u64,
}

#[derive(Debug, Deserialize)]
pub struct ContainerName {
    pub name: String,
}

// ---------------------------------------------------------------------------
// JSON error helper
// ---------------------------------------------------------------------------

fn json_error(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": msg })))
}

// ---------------------------------------------------------------------------
// Container data collection
// ---------------------------------------------------------------------------

fn common_filters() -> HashMap<String, Vec<String>> {
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
}

fn format_uptime(started_at: &str) -> String {
    let Ok(start) = chrono_parse(started_at) else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - start;
    if diff < 0 {
        return String::new();
    }
    let days = diff / 86400;
    let hours = (diff % 86400) / 3600;
    let mins = (diff % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

/// Minimal RFC3339 parser — returns Unix timestamp (seconds).
fn chrono_parse(s: &str) -> Result<i64, ()> {
    // Expected format: 2024-01-15T10:30:00.123456789Z (or +00:00 offset)
    let s = s.trim();
    if s.len() < 19 {
        return Err(());
    }
    let year: i64 = s[0..4].parse().map_err(|_| ())?;
    let month: i64 = s[5..7].parse().map_err(|_| ())?;
    let day: i64 = s[8..10].parse().map_err(|_| ())?;
    let hour: i64 = s[11..13].parse().map_err(|_| ())?;
    let min: i64 = s[14..16].parse().map_err(|_| ())?;
    let sec: i64 = s[17..19].parse().map_err(|_| ())?;

    // Rough days-since-epoch (good enough for uptime display)
    fn days_from_year(y: i64) -> i64 {
        365 * y + y / 4 - y / 100 + y / 400
    }
    let month_days: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let leap_adj = if month > 2 && is_leap { 1 } else { 0 };
    let epoch_days = days_from_year(year - 1) - days_from_year(1969)
        + month_days[(month - 1) as usize]
        + day
        - 1
        + leap_adj;
    Ok(epoch_days * 86400 + hour * 3600 + min * 60 + sec)
}

fn extract_ports(inspect: &bollard::models::ContainerInspectResponse) -> Vec<PortMapping> {
    let mut ports = Vec::new();
    let bindings = inspect
        .host_config
        .as_ref()
        .and_then(|hc| hc.port_bindings.as_ref());

    if let Some(bindings) = bindings {
        for (container_spec, host_bindings) in bindings {
            // container_spec is like "25565/tcp"
            let parts: Vec<&str> = container_spec.split('/').collect();
            let container_port: u16 = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
            let protocol = parts.get(1).unwrap_or(&"tcp").to_string();

            if let Some(Some(bindings_vec)) = host_bindings.as_ref().map(Some) {
                for binding in bindings_vec {
                    let host_port: u16 = binding
                        .host_port
                        .as_ref()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(0);
                    if host_port > 0 && container_port > 0 {
                        ports.push(PortMapping {
                            host_port,
                            container_port,
                            protocol: protocol.clone(),
                        });
                    }
                }
            }
        }
    }
    ports
}

fn find_minecraft_host_port(ports: &[PortMapping]) -> Option<u16> {
    ports
        .iter()
        .find(|p| p.container_port == 25565)
        .map(|p| p.host_port)
}

async fn ping_minecraft(host_port: u16, seed: &str) -> Option<MinecraftInfo> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut stream =
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", host_port)).await?;
        craftping::tokio::ping(&mut stream, "127.0.0.1", host_port).await
    })
    .await;

    match result {
        Ok(Ok(response)) => {
            let players: Vec<PlayerInfo> = response
                .sample
                .unwrap_or_default()
                .into_iter()
                .map(|p| PlayerInfo {
                    name: p.name,
                    x: None,
                    y: None,
                    z: None,
                })
                .collect();

            let mods = if let Some(mod_info) = &response.mod_info {
                mod_info
                    .mod_list
                    .iter()
                    .map(|m| ModEntry {
                        id: m.mod_id.clone(),
                        version: m.version.clone(),
                    })
                    .collect()
            } else if let Some(forge_data) = &response.forge_data {
                forge_data
                    .mods
                    .iter()
                    .map(|m| ModEntry {
                        id: m.mod_id.clone(),
                        version: m.mod_marker.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            };

            Some(MinecraftInfo {
                version: response.version,
                player_count: response.online_players as i32,
                max_players: response.max_players as i32,
                players,
                mods,
                seed: seed.to_string(),
            })
        }
        Ok(Err(e)) => {
            tracing::debug!("Minecraft ping failed on port {}: {}", host_port, e);
            None
        }
        Err(_) => {
            tracing::debug!("Minecraft ping timed out on port {}", host_port);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// RCON — fetch player coordinates
// ---------------------------------------------------------------------------

fn find_rcon_host_port(ports: &[PortMapping]) -> Option<u16> {
    ports
        .iter()
        .find(|p| p.container_port == 25575)
        .map(|p| p.host_port)
}

/// Parse RCON response from `/data get entity <player> Pos`.
/// Example: `PlayerName has the following entity data: [-123.45d, 64.0d, 789.12d]`
fn parse_pos_response(response: &str) -> Option<(f64, f64, f64)> {
    let bracket_start = response.find('[')?;
    let bracket_end = response.find(']')?;
    let inner = &response[bracket_start + 1..bracket_end];
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 3 {
        return None;
    }
    let x: f64 = parts[0].trim().trim_end_matches('d').parse().ok()?;
    let y: f64 = parts[1].trim().trim_end_matches('d').parse().ok()?;
    let z: f64 = parts[2].trim().trim_end_matches('d').parse().ok()?;
    Some((x, y, z))
}

async fn fetch_player_positions(
    rcon_port: u16,
    rcon_password: &str,
    players: &mut [PlayerInfo],
) {
    if players.is_empty() || rcon_password.is_empty() {
        return;
    }

    let conn_result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let stream =
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", rcon_port)).await?;
        rcon::Builder::new()
            .enable_minecraft_quirks(true)
            .handshake(stream, rcon_password)
            .await
    })
    .await;

    let mut conn = match conn_result {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            tracing::debug!("RCON connect failed on port {}: {}", rcon_port, e);
            return;
        }
        Err(_) => {
            tracing::debug!("RCON connect timed out on port {}", rcon_port);
            return;
        }
    };

    for player in players.iter_mut() {
        let cmd = format!("data get entity {} Pos", player.name);
        match tokio::time::timeout(std::time::Duration::from_secs(2), conn.cmd(&cmd)).await {
            Ok(Ok(response)) => {
                if let Some((x, y, z)) = parse_pos_response(&response) {
                    player.x = Some(x);
                    player.y = Some(y);
                    player.z = Some(z);
                }
            }
            Ok(Err(e)) => {
                tracing::debug!("RCON cmd failed for '{}': {}", player.name, e);
            }
            Err(_) => {
                tracing::debug!("RCON cmd timed out for '{}'", player.name);
            }
        }
    }
}

async fn collect_container_info(docker: &Docker, name: &str) -> Option<ContainerInfo> {
    // Inspect container
    let inspect = docker
        .inspect_container(name, None::<InspectContainerOptions>)
        .await
        .ok()?;

    let state_obj = inspect.state.as_ref();
    let state = state_obj
        .and_then(|s| s.status)
        .map(|s| format!("{:?}", s).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());

    let status_str = if state == "running" {
        state_obj
            .and_then(|s| s.health.as_ref())
            .and_then(|h| h.status)
            .map(|s| format!("{:?}", s).to_lowercase())
            .unwrap_or_else(|| state.clone())
    } else {
        state.clone()
    };

    let image = inspect
        .config
        .as_ref()
        .and_then(|c| c.image.clone())
        .unwrap_or_default();

    let created = inspect.created.clone().unwrap_or_default();

    let started_at = state_obj
        .and_then(|s| s.started_at.clone())
        .unwrap_or_default();

    let uptime = if state == "running" {
        format_uptime(&started_at)
    } else {
        String::new()
    };

    let restart_count = inspect.restart_count.unwrap_or(0);

    let ports = extract_ports(&inspect);

    let running = state == "running";

    // Only fetch stats for running containers — stats is expensive (cgroup reads)
    let (cpu_percent, memory_usage, memory_limit) = if running {
        get_container_stats(docker, name).await
    } else {
        (0.0, 0, 0)
    };

    // Extract env vars (itzg/minecraft-server convention)
    let envs = inspect
        .config
        .as_ref()
        .and_then(|c| c.env.as_ref());

    let seed = envs
        .and_then(|e| e.iter().find_map(|v| v.strip_prefix("SEED=").map(|s| s.to_string())))
        .unwrap_or_default();

    let rcon_password = envs
        .and_then(|e| e.iter().find_map(|v| v.strip_prefix("RCON_PASSWORD=").map(|s| s.to_string())))
        .unwrap_or_default();

    // Mark as MC container (has port 25565) — actual ping data merged from mc_cache later
    let is_mc = find_minecraft_host_port(&ports).is_some();
    let minecraft = if is_mc {
        Some(MinecraftInfo {
            version: String::new(),
            player_count: 0,
            max_players: 0,
            players: Vec::new(),
            mods: Vec::new(),
            seed,
        })
    } else {
        None
    };

    Some(ContainerInfo {
        name: name.to_string(),
        state,
        status: status_str,
        image,
        created,
        uptime,
        ports,
        cpu_percent,
        memory_usage,
        memory_limit,
        restart_count,
        minecraft,
        rcon_password,
    })
}

async fn get_container_stats(docker: &Docker, name: &str) -> (f64, u64, u64) {
    use futures_util::StreamExt;

    let stats_result = docker
        .stats(
            name,
            Some(StatsOptions {
                stream: false,
                one_shot: true,
            }),
        )
        .next()
        .await;

    match stats_result {
        Some(Ok(stats)) => {
            // CPU calculation
            let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as f64
                - stats.precpu_stats.cpu_usage.total_usage as f64;
            let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
                - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
            let num_cpus = stats
                .cpu_stats
                .cpu_usage
                .percpu_usage
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(1) as f64;

            let cpu_percent = if system_delta > 0.0 && cpu_delta > 0.0 {
                (cpu_delta / system_delta) * num_cpus * 100.0
            } else {
                0.0
            };

            let memory_usage = stats.memory_stats.usage.unwrap_or(0);
            let memory_limit = stats.memory_stats.limit.unwrap_or(0);

            (cpu_percent, memory_usage, memory_limit)
        }
        _ => (0.0, 0, 0),
    }
}

// ---------------------------------------------------------------------------
// Background refresh task
// ---------------------------------------------------------------------------

async fn background_refresh(state: SharedState) {
    let allowed_set: HashSet<&str> = state.allowed_containers.iter().map(|s| s.as_str()).collect();
    let mut mc_tick: u32 = 0; // counts 5s ticks; ping MC every 6 ticks (30s)

    loop {
        let docker = &state.docker;

        let options = ListContainersOptions {
            all: true,
            filters: state.common_filters.clone(),
            limit: Some(200),
            size: false,
        };

        let containers = match docker.list_containers(Some(options)).await {
            Ok(list) => list,
            Err(e) => {
                tracing::error!("Failed to list containers: {}", e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let names: Vec<String> = containers
            .iter()
            .filter_map(|c| {
                c.names.as_ref().and_then(|names| {
                    names
                        .first()
                        .map(|n| n.trim_start_matches('/').to_string())
                })
            })
            .filter(|n| allowed_set.contains(n.as_str()))
            .collect();

        let futures: Vec<_> = names
            .iter()
            .map(|name| collect_container_info(docker, name))
            .collect();
        let results = futures_util::future::join_all(futures).await;

        let mut all_infos: Vec<ContainerInfo> = results.into_iter().flatten().collect();

        let found_names: HashSet<String> = all_infos.iter().map(|i| i.name.clone()).collect();
        for name in &state.allowed_containers {
            if !found_names.contains(name) {
                if let Some(info) = collect_container_info(docker, name).await {
                    all_infos.push(info);
                }
            }
        }

        all_infos.sort_by(|a, b| a.name.cmp(&b.name));

        // MC ping on a 30s cycle (every 6th tick), or on first tick
        #[allow(clippy::manual_is_multiple_of)]
        let do_mc_ping = mc_tick % 6 == 0;
        if do_mc_ping {
            let mc_cache = state.mc_cache.read().await;
            // Determine which containers need a ping:
            // - must be running
            // - must be MC (has minecraft field)
            // - either first ping (not in cache) or has players connected
            // (container_name, mc_port, seed, rcon_port, rcon_password)
            let to_ping: Vec<(String, u16, String, Option<u16>, String)> = all_infos
                .iter()
                .filter(|c| c.state == "running" && c.minecraft.is_some())
                .filter_map(|c| {
                    find_minecraft_host_port(&c.ports).map(|port| {
                        let seed = c
                            .minecraft
                            .as_ref()
                            .map(|m| m.seed.clone())
                            .unwrap_or_default();
                        let rcon_port = find_rcon_host_port(&c.ports);
                        (c.name.clone(), port, seed, rcon_port, c.rcon_password.clone())
                    })
                })
                .filter(|(name, _, _, _, _)| {
                    match mc_cache.get(name.as_str()) {
                        None => true,
                        Some(cached) => cached.player_count > 0,
                    }
                })
                .collect();
            drop(mc_cache);

            for (name, port, seed, rcon_port, rcon_pass) in &to_ping {
                if let Some(mut mc_info) = ping_minecraft(*port, seed).await {
                    // Fetch player positions via RCON and log them
                    if let Some(rp) = rcon_port {
                        if !rcon_pass.is_empty() && !mc_info.players.is_empty() {
                            fetch_player_positions(*rp, rcon_pass, &mut mc_info.players).await;
                            for p in &mc_info.players {
                                if let (Some(x), Some(y), Some(z)) = (p.x, p.y, p.z) {
                                    tracing::info!(
                                        "[{}] Player '{}' at x={:.1} y={:.1} z={:.1}",
                                        name, p.name, x, y, z
                                    );
                                }
                            }
                        }
                    }
                    let mut mc_cache = state.mc_cache.write().await;
                    mc_cache.insert(name.clone(), mc_info);
                }
            }
        }
        mc_tick = mc_tick.wrapping_add(1);

        // Merge cached MC data into container infos
        {
            let mc_cache = state.mc_cache.read().await;
            for info in &mut all_infos {
                if info.minecraft.is_some() {
                    if let Some(cached) = mc_cache.get(&info.name) {
                        // Preserve seed from inspect, overlay ping data
                        let seed = info
                            .minecraft
                            .as_ref()
                            .map(|m| m.seed.clone())
                            .unwrap_or_default();
                        info.minecraft = Some(MinecraftInfo {
                            seed,
                            ..cached.clone()
                        });
                    }
                }
            }
        }

        {
            let mut cache = state.container_cache.write().await;
            *cache = all_infos;
        }
        state
            .cache_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn render_containers_html(State(state): State<SharedState>) -> Html<String> {
    let cache = state.container_cache.read().await;
    let containers: Vec<Container> = cache
        .iter()
        .map(|c| Container {
            name: c.name.clone(),
            status: c.status.clone(),
            state: c.state.clone(),
        })
        .collect();
    let template = ContainersTemplate {
        containers,
        port: state.port,
    };
    Html(template.render().unwrap_or_default())
}

async fn get_container_statuses(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let gen = state
        .cache_generation
        .load(std::sync::atomic::Ordering::Relaxed);
    let etag = format!("\"{}\"", gen);

    // If client already has this generation, skip the body
    if let Some(prev) = headers.get(axum::http::header::IF_NONE_MATCH) {
        if prev.as_bytes() == etag.as_bytes() {
            return (
                StatusCode::NOT_MODIFIED,
                [(axum::http::header::ETAG, etag)],
                String::new(),
            );
        }
    }

    let cache = state.container_cache.read().await;
    let body = serde_json::to_string(&*cache).unwrap_or_else(|_| "[]".to_string());
    (StatusCode::OK, [(axum::http::header::ETAG, etag)], body)
}

async fn start_container_handle(
    State(state): State<SharedState>,
    Query(query): Query<ContainerName>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    tracing::info!("Start request for '{}' from {}", query.name, addr);

    if !state.allowed_containers.contains(&query.name) {
        tracing::warn!("Container '{}' not allowed", query.name);
        return json_error(StatusCode::FORBIDDEN, "Container not allowed");
    }

    match state
        .docker
        .start_container(&query.name, None::<StartContainerOptions<String>>)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "started" })),
        ),
        Err(e) => {
            tracing::error!("Failed to start '{}': {}", query.name, e);
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to start container: {}", e),
            )
        }
    }
}

async fn stop_container_handle(
    State(state): State<SharedState>,
    Query(query): Query<ContainerName>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    tracing::info!("Stop request for '{}' from {}", query.name, addr);

    if !state.allowed_containers.contains(&query.name) {
        tracing::warn!("Container '{}' not allowed", query.name);
        return json_error(StatusCode::FORBIDDEN, "Container not allowed");
    }

    match state
        .docker
        .stop_container(&query.name, None::<StopContainerOptions>)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "stopped" })),
        ),
        Err(e) => {
            tracing::error!("Failed to stop '{}': {}", query.name, e);
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to stop container: {}", e),
            )
        }
    }
}

async fn stop_all_handle(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    tracing::info!("Stop-all request from {}", addr);

    let mut results = Vec::new();
    for name in &state.allowed_containers {
        tracing::info!("Stopping '{}'...", name);
        match state
            .docker
            .stop_container(name, None::<StopContainerOptions>)
            .await
        {
            Ok(_) => {
                tracing::info!("Stopped '{}'", name);
                results.push(serde_json::json!({ "name": name, "status": "stopped" }));
            }
            Err(e) => {
                tracing::error!("Failed to stop '{}': {}", name, e);
                results.push(serde_json::json!({ "name": name, "error": e.to_string() }));
            }
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "results": results })))
}

// ---------------------------------------------------------------------------
// Initialization helpers
// ---------------------------------------------------------------------------

async fn resolve_allowed_containers(args: &Args, docker: &Docker) -> Vec<String> {
    let containers_from_cli = args
        .containers
        .as_ref()
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok());

    if let Some(containers) = containers_from_cli {
        return containers;
    }

    let containers_from_file = load_file_containers(args.file.as_deref().unwrap_or("containers.txt"));

    // Get all container names from Docker to validate
    let options = ListContainersOptions {
        all: true,
        filters: common_filters(),
        limit: Some(200),
        size: false,
    };
    let docker_containers: HashSet<String> = docker
        .list_containers(Some(options))
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            c.names.as_ref().and_then(|names| {
                names
                    .first()
                    .map(|n| n.trim_start_matches('/').to_string())
            })
        })
        .collect();

    containers_from_file
        .into_iter()
        .filter(|c| docker_containers.contains(c))
        .collect()
}

fn load_file_containers(filename: &str) -> Vec<String> {
    if std::fs::metadata(filename).is_ok() {
        match read_to_string(filename) {
            Ok(content) => content.lines().map(|l| l.to_string()).collect(),
            Err(e) => {
                tracing::warn!("Failed to read {}: {}", filename, e);
                Vec::new()
            }
        }
    } else {
        tracing::warn!("File '{}' does not exist", filename);
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// CLI arguments
// ---------------------------------------------------------------------------

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

fn parse_log_level(log_level: &str) -> Level {
    match log_level.to_lowercase().as_str() {
        "error" => Level::ERROR,
        "warn" => Level::WARN,
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = parse_log_level(&args.log_level);
    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    let docker = Docker::connect_with_local_defaults().expect("Failed to connect to Docker");

    let allowed = resolve_allowed_containers(&args, &docker).await;
    tracing::info!("Allowed containers: {:?}", allowed);

    let state = Arc::new(AppState {
        docker,
        allowed_containers: allowed,
        port: args.port,
        container_cache: RwLock::new(Vec::new()),
        cache_generation: std::sync::atomic::AtomicU64::new(0),
        common_filters: common_filters(),
        mc_cache: RwLock::new(HashMap::new()),
    });

    // Spawn background refresh task
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            background_refresh(state).await;
        });
    }

    let app = Router::new()
        .route("/containers", get(render_containers_html))
        .route("/containers/statuses", get(get_container_statuses))
        .route("/containers/start", get(start_container_handle))
        .route("/containers/stop", get(stop_container_handle))
        .route("/containers/stop-all", get(stop_all_handle))
        .with_state(state)
        .into_make_service_with_connect_info::<SocketAddr>();

    tracing::info!("Starting docker-direct on port {}", args.port);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", args.port))
        .await
        .expect("Failed to bind listener");
    axum::serve(listener, app).await?;

    Ok(())
}
