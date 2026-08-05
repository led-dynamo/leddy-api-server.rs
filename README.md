# leddy-api-server.rs

Rust/Axum control plane for message publication, display clearing, connected
device discovery, command fan-out, and telemetry intake.

```sh
cargo run
# API listens on 0.0.0.0:8080 by default
```

Device agents connect to `/v1/ws/devices`; operator clients publish messages to
`POST /v1/messages`.
