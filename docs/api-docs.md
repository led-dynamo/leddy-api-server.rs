# Canonical API documentation

`leddy-api-server.rs` implements the fleet `ore.api-docs.v1` HTTP contract.

## Public same-origin routes

- `GET /.well-known/api-docs` — discovery, provenance, digest, and API/MCP pairing.
- `GET /openapi.json` — canonical public OpenAPI 3.1 document.
- `GET /api/docs.json` — exact-byte compatibility alias.
- `GET /api/docs` — static operation catalog.
- `GET /docs/api` — compatibility alias.

The documentation router is composed outside the permissive device API CORS layer. It receives normal request tracing but no device state, broadcast sender, desired display state, WebSocket session, or device telemetry.

## MCP pairing

The manifest pairs this API with canonical public repository `led-dynamo/leddy-mcp-server.rs`, owned by DEN-2885 and currently blocked by DEN-2884 plus the protected repository-publication credential path.

The baseline MCP repository must provide only:

- `api_docs_discover`
- `api_docs_get_openapi`
- `api_docs_validate`
- `api_docs_list_operations`
- `api_docs_describe_operation`

Only `GET /health` is MCP-exposed. The remaining operations are intentionally excluded:

- publishing a display message mutates desired state and broadcasts a command;
- clearing displays mutates desired state and broadcasts a command;
- device listing and lookup reveal identifiers, firmware, and telemetry-derived state;
- the device WebSocket is a long-lived bidirectional command and telemetry transport.

The WebSocket is a `GET` upgrade and therefore carries `x-ore-mcp-mutating=false` under the fleet HTTP-method classification, but it remains `x-ore-mcp-expose=false` and is never executable through the baseline MCP server.

## Promotion gate

The API PR remains draft until:

1. DEN-2884 clears the canonical MCP publication dependency;
2. `led-dynamo/leddy-mcp-server.rs` exists and uses the five read-only documentation tools;
3. source CI passes on immutable API and MCP heads;
4. `led-dynamo-test` verifies exact document parity, operation metadata, WebSocket exclusion, and zero executable device mutations;
5. tested heads remain unchanged at promotion.

This documentation standard requires no Cloudflare DNS, Worker, R2, origin, route, or secret change.
