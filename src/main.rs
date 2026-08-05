#![forbid(unsafe_code)]

use axum::{
    Json, Router,
    extract::{Path, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use leddy_interfaces::{DeviceCommand, DeviceEvent, MessageEnvelope};
use serde::Serialize;
use std::{collections::HashMap, env, sync::Arc, time::{SystemTime, UNIX_EPOCH}};
use tokio::sync::{RwLock, broadcast};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Clone)]
struct AppState {
    commands: broadcast::Sender<DeviceCommand>,
    devices: Arc<RwLock<HashMap<String, DeviceSnapshot>>>,
}

#[derive(Debug, Clone, Serialize)]
struct DeviceSnapshot {
    device_id: String,
    firmware_version: String,
    last_seen_unix_ms: u64,
    current_message_id: Option<String>,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let (commands, _) = broadcast::channel(256);
    let state = AppState {
        commands,
        devices: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/messages", post(publish_message))
        .route("/v1/clear", post(clear_displays))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{device_id}", get(get_device))
        .route("/v1/ws/devices", get(device_socket))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let address = env::var("LEDDY_API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&address).await.expect("bind API address");
    tracing::info!(%address, "Leddy API server listening");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await.expect("serve API");
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn publish_message(
    State(state): State<AppState>,
    Json(message): Json<MessageEnvelope>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    message.validate().map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let receivers = state.commands.send(DeviceCommand::Show(message.clone())).unwrap_or(0);
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({
        "message_id": message.id,
        "connected_receivers": receivers
    }))))
}

async fn clear_displays(State(state): State<AppState>) -> impl IntoResponse {
    let receivers = state.commands.send(DeviceCommand::Clear).unwrap_or(0);
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "connected_receivers": receivers })))
}

async fn list_devices(State(state): State<AppState>) -> Json<Vec<DeviceSnapshot>> {
    Json(state.devices.read().await.values().cloned().collect())
}

async fn get_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceSnapshot>, StatusCode> {
    state.devices.read().await.get(&device_id).cloned().map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn device_socket(
    websocket: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| serve_device(socket, state))
}

async fn serve_device(mut socket: WebSocket, state: AppState) {
    let mut commands = state.commands.subscribe();
    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Ok(command) => {
                        let Ok(json) = serde_json::to_string(&command) else { continue };
                        if socket.send(Message::Text(json.into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(event) = serde_json::from_str::<DeviceEvent>(&text) {
                            record_event(&state, event).await;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn record_event(state: &AppState, event: DeviceEvent) {
    let now = now_unix_ms();
    match event {
        DeviceEvent::Hello { device_id, firmware_version, .. } => {
            state.devices.write().await.insert(device_id.clone(), DeviceSnapshot {
                device_id,
                firmware_version,
                last_seen_unix_ms: now,
                current_message_id: None,
            });
        }
        DeviceEvent::Telemetry(telemetry) => {
            let mut devices = state.devices.write().await;
            let entry = devices.entry(telemetry.device_id.clone()).or_insert(DeviceSnapshot {
                device_id: telemetry.device_id.clone(),
                firmware_version: "unknown".into(),
                last_seen_unix_ms: now,
                current_message_id: None,
            });
            entry.last_seen_unix_ms = now;
            entry.current_message_id = telemetry.current_message_id;
        }
        _ => {}
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
