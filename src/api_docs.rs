//! Canonical public API-documentation routes and discovery manifest.
//!
//! The baseline catalog describes display and device routes but never executes
//! display commands, opens device WebSockets, or returns device telemetry.

use axum::{
    Router,
    body::Body,
    http::{Response, StatusCode, header},
    routing::get,
};

pub const DISCOVERY_PATH: &str = "/.well-known/api-docs";
pub const OPENAPI_PATH: &str = "/openapi.json";
pub const OPENAPI_ALIAS: &str = "/api/docs.json";
pub const DOCS_PATH: &str = "/api/docs";
pub const DOCS_ALIAS: &str = "/docs/api";
pub const OPENAPI_SHA256: &str = "cf0be66ce0ebb02c3fc077a88c3129c55b4d05f30070b3c7186d13731ae7fe88";

const OPENAPI_ETAG: &str = "\"cf0be66ce0ebb02c3fc077a88c3129c55b4d05f30070b3c7186d13731ae7fe88\"";
const OPENAPI_MEDIA_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";
const OPENAPI: &str = include_str!("../openapi/leddy.openapi.json");
const MANIFEST: &str = include_str!("../openapi/api-docs.manifest.json");
const DOCS_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Leddy Device API documentation</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { margin: 0 auto; max-width: 72rem; padding: 2rem; }
    table { border-collapse: collapse; width: 100%; }
    th, td { border-bottom: 1px solid currentColor; padding: .55rem; text-align: left; }
    .mutating { font-weight: 700; }
  </style>
</head>
<body>
  <h1>Leddy Device API</h1>
  <p id="provenance">Loading the canonical OpenAPI contract…</p>
  <p>The baseline MCP pairing describes routes only. It cannot publish messages, clear displays, connect devices, or read device telemetry.</p>
  <table>
    <thead><tr><th>Method</th><th>Path</th><th>Operation</th><th>Summary</th><th>MCP catalog</th></tr></thead>
    <tbody id="operations"></tbody>
  </table>
  <script>
    (async () => {
      const manifestResponse = await fetch('/.well-known/api-docs', {redirect: 'error'});
      if (!manifestResponse.ok) throw new Error('manifest unavailable');
      const manifest = await manifestResponse.json();
      const specResponse = await fetch(manifest.public.openapi.path, {redirect: 'error'});
      if (!specResponse.ok) throw new Error('OpenAPI unavailable');
      const spec = await specResponse.json();
      document.querySelector('#provenance').textContent =
        `${spec.info.title} ${spec.info.version} · SHA-256 ${manifest.public.openapi.sha256}`;
      const rows = [];
      for (const [path, item] of Object.entries(spec.paths)) {
        for (const method of ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace']) {
          const operation = item[method];
          if (!operation) continue;
          rows.push({path, method: method.toUpperCase(), operation});
        }
      }
      rows.sort((a, b) => a.operation.operationId.localeCompare(b.operation.operationId));
      const body = document.querySelector('#operations');
      for (const row of rows) {
        const tr = document.createElement('tr');
        if (row.operation['x-ore-mcp-mutating']) tr.className = 'mutating';
        const values = [
          row.method,
          row.path,
          row.operation.operationId,
          row.operation.summary,
          row.operation['x-ore-mcp-expose'] ? 'read-only metadata' : 'not exposed'
        ];
        for (const value of values) {
          const td = document.createElement('td');
          td.textContent = value;
          tr.appendChild(td);
        }
        body.appendChild(tr);
      }
    })().catch((error) => {
      document.querySelector('#provenance').textContent = `Documentation error: ${error.message}`;
    });
  </script>
</body>
</html>
"#;

pub fn router() -> Router {
    Router::new()
        .route(DISCOVERY_PATH, get(discovery))
        .route(OPENAPI_PATH, get(openapi))
        .route(OPENAPI_ALIAS, get(openapi))
        .route(DOCS_PATH, get(docs))
        .route(DOCS_ALIAS, get(docs))
}

async fn discovery() -> Response<Body> {
    response(MANIFEST, "application/json", false, false)
}

async fn openapi() -> Response<Body> {
    response(OPENAPI, OPENAPI_MEDIA_TYPE, false, true)
}

async fn docs() -> Response<Body> {
    response(DOCS_HTML, "text/html; charset=utf-8", true, false)
}

fn response(
    body: &'static str,
    content_type: &'static str,
    html: bool,
    openapi_etag: bool,
) -> Response<Body> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=300")
        .header("x-openapi-sha256", OPENAPI_SHA256)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if openapi_etag {
        builder = builder.header(header::ETAG, OPENAPI_ETAG);
    }
    if html {
        builder = builder.header(
            "content-security-policy",
            "default-src 'none'; connect-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'",
        );
    }
    builder
        .body(Body::from(body))
        .expect("static API documentation response must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Method, Request},
    };
    use serde_json::Value;
    use tower::ServiceExt;

    async fn fetch(path: &str, method: Method) -> Response<Body> {
        router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .expect("request must build"),
            )
            .await
            .expect("documentation router must respond")
    }

    #[tokio::test]
    async fn openapi_alias_is_exact_byte_for_byte() {
        let canonical = fetch(OPENAPI_PATH, Method::GET).await;
        let alias = fetch(OPENAPI_ALIAS, Method::GET).await;
        assert_eq!(canonical.status(), StatusCode::OK);
        assert_eq!(alias.status(), StatusCode::OK);
        assert_eq!(
            canonical
                .headers()
                .get("x-openapi-sha256")
                .expect("digest header")
                .to_str()
                .expect("digest header text"),
            OPENAPI_SHA256
        );
        let canonical = to_bytes(canonical.into_body(), 1024 * 1024)
            .await
            .expect("canonical body must be bounded");
        let alias = to_bytes(alias.into_body(), 1024 * 1024)
            .await
            .expect("alias body must be bounded");
        assert_eq!(canonical, alias);
        assert_eq!(canonical.as_ref(), OPENAPI.as_bytes());
    }

    #[tokio::test]
    async fn manifest_names_canonical_pair_and_head_is_empty() {
        let response = fetch(DISCOVERY_PATH, Method::GET).await;
        let body = to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("manifest body must be bounded");
        let manifest: Value = serde_json::from_slice(&body).expect("manifest must be JSON");
        assert_eq!(manifest["schemaVersion"], "ore.api-docs.v1");
        assert_eq!(manifest["public"]["openapi"]["sha256"], OPENAPI_SHA256);
        assert_eq!(
            manifest["mcp"]["repository"],
            "led-dynamo/leddy-mcp-server.rs"
        );
        assert_eq!(manifest["mcp"]["mode"], "read-only");
        assert_eq!(manifest["internal"]["available"], false);

        let head = fetch(OPENAPI_PATH, Method::HEAD).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()
                .get(header::CONTENT_TYPE)
                .expect("content type")
                .to_str()
                .expect("content type text"),
            OPENAPI_MEDIA_TYPE
        );
        assert!(
            to_bytes(head.into_body(), 1)
                .await
                .expect("HEAD response must be empty")
                .is_empty()
        );
    }

    #[test]
    fn openapi_covers_routes_and_exposes_only_health() {
        let source = include_str!("main.rs");
        for fragment in [
            r#".route("/health", get(health))"#,
            r#".route("/v1/messages", post(publish_message))"#,
            r#".route("/v1/clear", post(clear_displays))"#,
            r#".route("/v1/devices", get(list_devices))"#,
            r#".route("/v1/devices/{device_id}", get(get_device))"#,
            r#".route("/v1/ws/devices", get(device_socket))"#,
        ] {
            assert!(
                source.contains(fragment),
                "missing route source fragment {fragment}"
            );
        }

        let value: Value = serde_json::from_str(OPENAPI).expect("OpenAPI must be JSON");
        assert_eq!(value["openapi"], "3.1.0");
        let paths = value["paths"].as_object().expect("paths must be an object");
        let expected = [
            ("/health", "get"),
            ("/v1/messages", "post"),
            ("/v1/clear", "post"),
            ("/v1/devices", "get"),
            ("/v1/devices/{device_id}", "get"),
            ("/v1/ws/devices", "get"),
        ];
        let mut operation_ids = std::collections::BTreeSet::new();
        let mut exposed = Vec::new();
        for (path, method) in expected {
            let operation = &paths[path][method];
            let operation_id = operation["operationId"]
                .as_str()
                .expect("operationId must be a string");
            assert!(operation_ids.insert(operation_id.to_owned()));
            assert_eq!(operation["x-ore-visibility"], "public");
            let mutating = operation["x-ore-mcp-mutating"]
                .as_bool()
                .expect("mutation flag must be Boolean");
            let mcp_expose = operation["x-ore-mcp-expose"]
                .as_bool()
                .expect("exposure flag must be Boolean");
            assert_eq!(mutating, !matches!(method, "get" | "head" | "options"));
            assert!(!(mutating && mcp_expose));
            if mcp_expose {
                exposed.push(operation_id);
            }
        }
        assert_eq!(paths.len(), 6);
        assert_eq!(operation_ids.len(), 6);
        assert_eq!(exposed, ["getHealth"]);
    }
}
