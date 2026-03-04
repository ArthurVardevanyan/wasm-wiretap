use serde::Deserialize;

/// The backend to export captured telemetry to.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExporterBackend {
    Loki,
    Otlp,
}

impl Default for ExporterBackend {
    fn default() -> Self {
        ExporterBackend::Otlp
    }
}

/// Top-level plugin configuration, supplied via the Istio WasmPlugin
/// `pluginConfig` field (JSON).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Which exporter backend to use: "loki" or "otlp" (default).
    pub backend: ExporterBackend,

    /// The Envoy cluster name (or authority) that routes to the
    /// telemetry collector.  This is resolved by Envoy's cluster
    /// manager and must match a `ServiceEntry` or real upstream.
    ///
    /// Example: "loki.monitoring.svc.cluster.local"
    pub upstream_cluster: String,

    /// Authority (Host header) to use when calling the upstream.
    pub upstream_authority: String,

    /// Path for the push endpoint.
    /// - Loki default:  `/loki/api/v1/push`
    /// - OTLP default:  `/v1/logs`
    pub upstream_path: String,

    /// Port of the upstream service (used in the authority header).
    pub upstream_port: u32,

    /// Optional static labels attached to every export (Loki) or
    /// resource attributes (OTLP).
    pub labels: std::collections::HashMap<String, String>,

    /// Service name label (maps to `service_name` in OTel,
    /// `service` label in Loki).
    pub service_name: String,

    /// Whether to capture request headers.
    pub capture_request_headers: bool,

    /// Whether to capture request body.
    pub capture_request_body: bool,

    /// Whether to capture response headers.
    pub capture_response_headers: bool,

    /// Whether to capture response body.
    pub capture_response_body: bool,

    /// Maximum body bytes to capture (per direction). 0 = unlimited.
    pub max_body_bytes: usize,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            backend: ExporterBackend::Otlp,
            upstream_cluster: String::new(),
            upstream_authority: String::new(),
            upstream_path: String::new(),
            upstream_port: 0,
            labels: std::collections::HashMap::new(),
            service_name: "wasm-wiretap".to_string(),
            capture_request_headers: true,
            capture_request_body: true,
            capture_response_headers: true,
            capture_response_body: true,
            max_body_bytes: 65536, // 64 KiB
        }
    }
}

impl PluginConfig {
    /// Parse the JSON configuration blob supplied by Envoy / Istio.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let mut cfg: PluginConfig = serde_json::from_slice(bytes)?;

        // Apply sensible defaults for the push path when not explicitly set.
        if cfg.upstream_path.is_empty() {
            cfg.upstream_path = match cfg.backend {
                ExporterBackend::Loki => "/loki/api/v1/push".to_string(),
                ExporterBackend::Otlp => "/v1/logs".to_string(),
            };
        }

        Ok(cfg)
    }

    /// Build the authority value (host:port) used in dispatch_http_call.
    pub fn authority(&self) -> String {
        if self.upstream_port > 0 {
            format!("{}:{}", self.upstream_authority, self.upstream_port)
        } else {
            self.upstream_authority.clone()
        }
    }
}
