# leddy-api-server.rs

Rust/Axum control plane for message publication, display clearing, connected
device discovery, command fan-out, and telemetry intake.

```sh
cargo run
# API listens on 0.0.0.0:8080 by default
```

Device agents connect to `/v1/ws/devices`; operator clients publish messages to
`POST /v1/messages` and clear displays with `POST /v1/clear`.

## Desired-state and reconnect contract

Every accepted display mutation receives a monotonically increasing in-memory
revision. The API stores the latest desired command (`show` or `clear`) before
broadcasting it to connected devices.

A device WebSocket subscribes to the live command stream before reading the
latest desired state. On connection or after broadcast lag, the server sends
the latest desired command when its revision is newer than the last revision
already sent on that socket. If the same revision is also waiting in the live
broadcast queue, it is skipped.

This gives reconnecting devices the current display state without replaying the
same active mutation twice. Revisions are intentionally internal transport
metadata: the public `DeviceCommand` schema remains shared through
`leddy-interfaces`.

`POST /v1/messages` and `POST /v1/clear` include the accepted `revision` and
current `connected_receivers` count in their JSON responses.

Device `hello` and telemetry events populate `/v1/devices`; acknowledgements
received after `hello` update `last_ack_command_id` for that connection.
