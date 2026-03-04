# wasm-wiretap

A **proxy-wasm** plugin (Rust → Wasm) that passively captures HTTP request/response headers and bodies flowing through an Envoy-based proxy and exports them **asynchronously** to **Grafana Loki** or an **OpenTelemetry (OTLP/HTTP)** endpoint.

Designed for use with **Istio's Gateway API** implementation via the `WasmPlugin` CRD.

## Features

| Feature              | Details                                                              |
| -------------------- | -------------------------------------------------------------------- |
| **Request capture**  | Headers, body (with configurable size limit)                         |
| **Response capture** | Headers, body, status code                                           |
| **Async export**     | Uses Envoy's `dispatch_http_call` – the data-path is _never_ blocked |
| **Loki backend**     | Push API (`/loki/api/v1/push`) with custom labels                    |
| **OTLP backend**     | OTLP/HTTP JSON logs (`/v1/logs`) with resource attributes            |
| **Metadata**         | Request ID, source/destination address, duration, upstream cluster   |
| **Gateway API**      | Ready-made `WasmPlugin` manifests using `targetRefs`                 |

## Project Structure

```text
wasm-wiretap/
├── Cargo.toml
├── Dockerfile              # Multi-stage: builds .wasm in OCI image
├── Makefile
├── rust-toolchain.toml
├── src/
│   ├── lib.rs              # RootContext + HttpContext (main plugin)
│   ├── config.rs           # JSON configuration parsing
│   ├── capture.rs          # CapturedData struct
│   └── exporters.rs        # Loki & OTLP payload builders
└── deploy/
    ├── wasmplugin-loki.yaml    # Example: Loki on a Gateway
    ├── wasmplugin-otlp.yaml    # Example: OTLP on a Gateway
    └── wasmplugin-sidecar.yaml # Example: sidecar workload
```

## Prerequisites

- **Rust** (stable, 1.70+)
- `wasm32-wasip1` target:

  ```bash
  rustup target add wasm32-wasip1
  ```

- **Istio** 1.22+ with Gateway API support (for deployment)
- An OCI registry to push the Wasm image (e.g. `ghcr.io`, Docker Hub)

## Build

```bash
# Native build
make build          # release
make build-debug    # debug

# The .wasm binary lands at:
#   target/wasm32-wasip1/release/wasm_wiretap.wasm

# Copy to dist/ for convenience
make dist
```

### Docker / OCI Image

```bash
docker build -t ghcr.io/<you>/wasm-wiretap:latest .
docker push ghcr.io/<you>/wasm-wiretap:latest
```

The image contains a single file at `/plugin.wasm` – this is what Istio pulls.

## Configuration

The plugin is configured via the `pluginConfig` field in the Istio `WasmPlugin` resource. All fields are optional and have sensible defaults.

| Field                      | Type                 | Default          | Description                                                                                       |
| -------------------------- | -------------------- | ---------------- | ------------------------------------------------------------------------------------------------- |
| `backend`                  | `"loki"` \| `"otlp"` | `"otlp"`         | Export backend                                                                                    |
| `upstream_cluster`         | string               | `""`             | Envoy cluster name for the collector (e.g. `outbound\|3100\|\|loki.monitoring.svc.cluster.local`) |
| `upstream_authority`       | string               | `""`             | Host header for the upstream                                                                      |
| `upstream_port`            | int                  | `0`              | Port (appended to authority if > 0)                                                               |
| `upstream_path`            | string               | auto             | Push path (auto-detected from backend)                                                            |
| `service_name`             | string               | `"wasm-wiretap"` | Service name label / resource attribute                                                           |
| `capture_request_headers`  | bool                 | `true`           | Capture request headers                                                                           |
| `capture_request_body`     | bool                 | `true`           | Capture request body                                                                              |
| `capture_response_headers` | bool                 | `true`           | Capture response headers                                                                          |
| `capture_response_body`    | bool                 | `true`           | Capture response body                                                                             |
| `max_body_bytes`           | int                  | `65536`          | Max body bytes to capture per direction (0 = unlimited)                                           |
| `labels`                   | map                  | `{}`             | Extra labels (Loki) / resource attributes (OTLP)                                                  |

### Example: Loki

```json
{
  "backend": "loki",
  "upstream_cluster": "outbound|3100||loki.monitoring.svc.cluster.local",
  "upstream_authority": "loki.monitoring.svc.cluster.local",
  "upstream_port": 3100,
  "service_name": "my-gateway",
  "labels": { "env": "production" }
}
```

### Example: OpenTelemetry

```json
{
  "backend": "otlp",
  "upstream_cluster": "outbound|4318||otel-collector.monitoring.svc.cluster.local",
  "upstream_authority": "otel-collector.monitoring.svc.cluster.local",
  "upstream_port": 4318,
  "service_name": "my-gateway"
}
```

## Deploy to Istio (Gateway API)

### 1. Create a `ServiceEntry` for the collector

Envoy needs to know how to reach the telemetry backend. If it's already a Kubernetes Service in the mesh you can skip this.

```yaml
apiVersion: networking.istio.io/v1
kind: ServiceEntry
metadata:
  name: loki-external
  namespace: istio-system
spec:
  hosts: ["loki.monitoring.svc.cluster.local"]
  ports:
    - number: 3100
      name: http
      protocol: HTTP
  resolution: DNS
  location: MESH_INTERNAL
```

### 2. Apply the WasmPlugin

```bash
# Loki
kubectl apply -f deploy/wasmplugin-loki.yaml

# — or — OpenTelemetry
kubectl apply -f deploy/wasmplugin-otlp.yaml
```

### 3. Verify

```bash
# Check plugin status
kubectl get wasmplugin -n istio-system

# Tail the gateway proxy logs for plugin output
kubectl logs -n istio-system deploy/my-gateway -c istio-proxy -f | grep wasm-wiretap
```

## How It Works

```text
  Client                   Envoy (Gateway)                Plugin
    │                           │                           │
    ├──── HTTP Request ────────►│                           │
    │                           ├─ on_http_request_headers ─►│ capture headers
    │                           ├─ on_http_request_body ────►│ buffer body
    │                           │                           │
    │                           │◄──── upstream response ───│
    │                           ├─ on_http_response_headers ►│ capture headers + status
    │                           ├─ on_http_response_body ───►│ buffer body
    │                           │                           │
    │◄─── HTTP Response ────────│                           │
    │                           │   (end of stream)         │
    │                           │                           ├─► dispatch_http_call()
    │                           │                           │   (async, non-blocking)
    │                           │                           │
    │                           │                           │──► Loki / OTel Collector
```

- **`dispatch_http_call`** is Envoy's built-in async HTTP mechanism. It fires the export request on a **side channel** and invokes `on_http_call_response` when the collector responds. The main request/response flow is **never paused or delayed**.
- If the export call fails or times out (5 s), the plugin logs a warning but the data-path traffic is unaffected.
- **Why request + response appear in the same log entry:** Envoy creates a single plugin instance (`WiretapHttp`) per HTTP transaction. That instance's `CapturedData` struct accumulates data across all four callbacks (`on_http_request_headers` → `on_http_request_body` → `on_http_response_headers` → `on_http_response_body`). Each callback returns `Action::Continue` immediately — the proxy keeps forwarding bytes without waiting. Only once the response is fully received does the plugin serialize the complete struct and export it via `dispatch_http_call`. So **capture is synchronous** (same context, same struct) while **export is asynchronous** (non-blocking side-channel call).

### WebSocket connections

For WebSocket upgrades (`Upgrade: websocket` → `101 Switching Protocols`), be aware of the following:

- **Handshake headers are captured** — the initial HTTP upgrade request and 101 response headers are recorded normally.
- **No frame data** — once the connection upgrades, Envoy bypasses the HTTP filter chain. `on_http_request_body` / `on_http_response_body` are never called for WebSocket frames, so `request_body` and `response_body` will be `null`.
- **Delayed export** — the `end_of_stream` signal never arrives during the connection's lifetime, so the log entry is only exported when the WebSocket closes (via the `on_log` callback). For long-lived connections this could be hours or days.
- **Memory impact is minimal** — with no body data being buffered, each open WebSocket holds only a few hundred bytes for the header metadata.
- **Duration** — `duration_ms` will reflect the entire WebSocket connection lifetime, not a single request.

### gRPC

gRPC runs over HTTP/2, so the Envoy HTTP filter chain stays active and all callbacks fire normally. However there are some nuances:

- **Headers captured correctly** — you'll see `:method: POST`, `:path: /package.Service/Method`, `content-type: application/grpc`, etc.
- **Bodies are binary protobuf** — the plugin converts body bytes via `String::from_utf8_lossy`, which produces garbled/lossy output for protobuf payloads. The data is captured but not human-readable. (A future option could base64-encode binary bodies instead.)
- **Status codes** — gRPC application errors use trailers (`grpc-status`, `grpc-message`), not the HTTP `:status` header, which is typically `200` even when the RPC fails. The plugin records `:status`, so most gRPC entries will show `status_code: 200` regardless of the actual gRPC result. Response trailers do appear in `response_headers` if captured.
- **Streaming RPCs** — unlike WebSockets, HTTP/2 streams keep the filter chain active:
  - **Unary** — works like a normal request/response. Single body in each direction, exported on completion.
  - **Server-streaming / Client-streaming** — body chunks accumulate up to `max_body_bytes` per direction. Export fires when the stream ends.
  - **Bidirectional streaming** — body callbacks fire for each frame, but long-lived bidi streams behave similarly to WebSockets: export is delayed until the stream closes, and `duration_ms` reflects the full stream lifetime.

### GraphQL

GraphQL is standard HTTP (typically `POST` with a JSON body), so the plugin handles it with **no special behaviour**:

- **Request body** — the JSON query/mutation (e.g. `{"query": "{ users { id name } }"}`) is captured as readable text.
- **Response body** — the JSON response is captured as-is.
- **Large responses** — complex queries returning large result sets will be truncated at `max_body_bytes` (default 64 KiB). Increase the limit or set to `0` (unlimited) if you need the full payload.

## Development

```bash
# Check compilation
make check

# Lint
make clippy

# Format
cargo fmt
```

## License

[MIT](LICENSE)
