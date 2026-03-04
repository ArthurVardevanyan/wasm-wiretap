use serde::Serialize;

/// Holds all captured data for a single HTTP transaction.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CapturedData {
    /// ISO-8601 timestamp (nanosecond precision) of when the request
    /// started flowing through the filter.
    pub timestamp_ns: u64,

    /// Unique request id (from x-request-id header, if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Request method (GET, POST, …).
    pub method: String,

    /// Request path.
    pub path: String,

    /// Request authority / host.
    pub authority: String,

    /// Captured request headers (name→value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Vec<(String, String)>>,

    /// Captured request body (base64 or UTF-8 depending on content
    /// type). We store as String; binary bodies are base64-encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,

    /// Captured response headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Vec<(String, String)>>,

    /// Captured response body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,

    /// HTTP status code of the response.
    pub status_code: u32,

    /// The upstream cluster that handled the request (from
    /// Envoy metadata).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_cluster: Option<String>,

    /// Duration of the request→response in milliseconds.
    pub duration_ms: u64,

    /// Source (downstream) address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_address: Option<String>,

    /// Destination (upstream) address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_address: Option<String>,
}

impl CapturedData {
    pub fn new() -> Self {
        Self::default()
    }
}
