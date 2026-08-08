#![forbid(unsafe_code)]

use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use leddy_interfaces::{DeviceCommand, DeviceEvent, MessageEnvelope};
use serde::Serialize;
use std::{
    collections::HashMap,
    env,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{RwLock, broadcast};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Debug, Clone)]
struct SequencedCommand {
    revision: u64,
    command: DeviceCommand,
}

#[derive(Debug, Clone, Default)]
struct DesiredDisplayState {
    revision: u64,
    command: Option<DeviceCommand>,
}

#[derive(Clone)]
struct AppState {
    commands: broadcast::Sender<SequencedCommand>,
    desired: Arc<RwLock<DesiredDisplayState>>,
    devices: Arc<RwLock<HashMap<String, DeviceSnapshot>>>,
}

impl AppState {
    fn new() -> Self {
        let (commands, _) = broadcast::channel(256);
        Self {
            commands,
            desired: Arc::new(RwLock::new(DesiredDisplayState::default())),
            devices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn publish(&self, command: DeviceCommand) -> (u64, usize) {
        let revision = {
            let mut desired = self.desired.write().await;
            desired.revision = desired.revision.saturating_add(1);
            desired.command = Some(command.clone());
            desired.revision
        };

        let receivers = self
            .commands
            .send(SequencedCommand { revision, command })
            .unwrap_or(0);
        (revision, receivers)
    }

    async fn desired_snapshot(&self) -> DesiredDisplayState {
        self.desired.read().await.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
struct DeviceSnapshot {
    device_id: String,
    firmware_version: String,
    last_seen_unix_ms: u64,
    current_message_id: Option<String>,
    last_ack_command_id: Option<String>,
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

    let state = AppState::new();
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
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("bind API address");
    tracing::info!(%address, "Leddy API server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("serve API");
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn publish_message(
    State(state): State<AppState>,
    Json(message): Json<MessageEnvelope>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    message
        .validate()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let message_id = message.id.clone();
    let (revision, receivers) = state.publish(DeviceCommand::Show(message)).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "message_id": message_id,
            "revision": revision,
            "connected_receivers": receivers
        })),
    ))
}

async fn clear_displays(State(state): State<AppState>) -> impl IntoResponse {
    let (revision, receivers) = state.publish(DeviceCommand::Clear).await;
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "revision": revision,
            "connected_receivers": receivers
        })),
    )
}

async fn list_devices(State(state): State<AppState>) -> Json<Vec<DeviceSnapshot>> {
    Json(state.devices.read().await.values().cloned().collect())
}

async fn get_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceSnapshot>, StatusCode> {
    state
        .devices
        .read()
        .await
        .get(&device_id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn device_socket(
    websocket: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| serve_device(socket, state))
}

async fn send_command(socket: &mut WebSocket, command: &DeviceCommand) -> bool {
    let Ok(json) = serde_json::to_string(command) else {
        return true;
    };
    socket.send(Message::Text(json.into())).await.is_ok()
}

async fn send_latest_if_newer(
    socket: &mut WebSocket,
    state: &AppState,
    last_sent_revision: &mut u64,
) -> bool {
    let desired = state.desired_snapshot().await;
    if desired.revision <= *last_sent_revision {
        return true;
    }
    let Some(command) = desired.command else {
        *last_sent_revision = desired.revision;
        return true;
    };
    if !send_command(socket, &command).await {
        return false;
    }
    *last_sent_revision = desired.revision;
    true
}

async fn serve_device(mut socket: WebSocket, state: AppState) {
    // Subscribe before reading desired state. If a mutation races with the snapshot,
    // its broadcast entry is queued and revision de-duplication prevents a duplicate.
    let mut commands = state.commands.subscribe();
    let mut last_sent_revision = 0_u64;
    let mut connection_device_id: Option<String> = None;

    if !send_latest_if_newer(&mut socket, &state, &mut last_sent_revision).await {
        return;
    }

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Ok(sequenced) => {
                        if sequenced.revision <= last_sent_revision {
                            continue;
                        }
                        if !send_command(&mut socket, &sequenced.command).await {
                            break;
                        }
                        last_sent_revision = sequenced.revision;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !send_latest_if_newer(&mut socket, &state, &mut last_sent_revision).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(event) = serde_json::from_str::<DeviceEvent>(&text) {
                            if let DeviceEvent::Hello { device_id, .. } = &event {
                                connection_device_id = Some(device_id.clone());
                            }
                            record_event(&state, event, connection_device_id.as_deref()).await;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn record_event(state: &AppState, event: DeviceEvent, connection_device_id: Option<&str>) {
    let now = now_unix_ms();
    match event {
        DeviceEvent::Hello {
            device_id,
            firmware_version,
            ..
        } => {
            state.devices.write().await.insert(
                device_id.clone(),
                DeviceSnapshot {
                    device_id,
                    firmware_version,
                    last_seen_unix_ms: now,
                    current_message_id: None,
                    last_ack_command_id: None,
                },
            );
        }
        DeviceEvent::Ack { command_id } => {
            if let Some(device_id) = connection_device_id
                && let Some(device) = state.devices.write().await.get_mut(device_id)
            {
                device.last_seen_unix_ms = now;
                device.last_ack_command_id = Some(command_id);
            }
        }
        DeviceEvent::Telemetry(telemetry) => {
            let mut devices = state.devices.write().await;
            let entry = devices
                .entry(telemetry.device_id.clone())
                .or_insert(DeviceSnapshot {
                    device_id: telemetry.device_id.clone(),
                    firmware_version: "unknown".into(),
                    last_seen_unix_ms: now,
                    current_message_id: None,
                    last_ack_command_id: None,
                });
            entry.last_seen_unix_ms = now;
            entry.current_message_id = telemetry.current_message_id;
        }
        _ => {}
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use leddy_interfaces::{RepeatMode, ScrollDirection};

    fn test_message(id: &str) -> MessageEnvelope {
        MessageEnvelope {
            id: id.into(),
            text: "HELLO LEDDY".into(),
            speed_pixels_per_second: 24.0,
            direction: ScrollDirection::Left,
            repeat: RepeatMode::Forever,
            issued_at_unix_ms: 1,
        }
    }

    #[tokio::test]
    async fn revisions_are_monotonic_and_latest_state_is_replayable() {
        let state = AppState::new();
        let (first_revision, _) = state
            .publish(DeviceCommand::Show(test_message("message-1")))
            .await;
        let (second_revision, _) = state
            .publish(DeviceCommand::Show(test_message("message-2")))
            .await;

        assert_eq!(first_revision, 1);
        assert_eq!(second_revision, 2);
        let desired = state.desired_snapshot().await;
        assert_eq!(desired.revision, 2);
        assert_eq!(
            desired.command,
            Some(DeviceCommand::Show(test_message("message-2")))
        );
    }

    #[tokio::test]
    async fn clear_becomes_the_reconnect_state() {
        let state = AppState::new();
        state
            .publish(DeviceCommand::Show(test_message("message-1")))
            .await;
        let (clear_revision, _) = state.publish(DeviceCommand::Clear).await;

        let desired = state.desired_snapshot().await;
        assert_eq!(clear_revision, 2);
        assert_eq!(desired.revision, 2);
        assert_eq!(desired.command, Some(DeviceCommand::Clear));
    }

    #[tokio::test]
    async fn subscribers_receive_revisioned_mutations() {
        let state = AppState::new();
        let mut receiver = state.commands.subscribe();
        let (revision, receivers) = state
            .publish(DeviceCommand::Show(test_message("message-1")))
            .await;

        assert_eq!(revision, 1);
        assert_eq!(receivers, 1);
        let received = receiver.recv().await.expect("command broadcast");
        assert_eq!(received.revision, 1);
        assert_eq!(
            received.command,
            DeviceCommand::Show(test_message("message-1"))
        );
    }
}
