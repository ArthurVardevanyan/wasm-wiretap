use std::collections::HashMap;

use crate::capture::CapturedData;
use crate::config::{ExporterBackend, PluginConfig};

// ───────────────────────────── Public API ─────────────────────────────

/// Build the HTTP headers + body payload for the configured exporter.
/// Returns `(headers, body_bytes)`.
pub fn build_export_payload(
    config: &PluginConfig,
    data: &CapturedData,
) -> (Vec<(String, String)>, Vec<u8>) {
    match config.backend {
        ExporterBackend::Loki => build_loki_payload(config, data),
        ExporterBackend::Otlp => build_otlp_payload(config, data),
    }
}

// ───────────────────────── Loki push payload ──────────────────────────

/// Build a Loki push-API JSON payload.
///
/// ```json
/// {
///   "streams": [{
///     "stream": { "service": "my-svc", ... },
///     "values": [
///       [ "<unix_epoch_ns>", "<json_line>" ]
///     ]
///   }]
/// }
/// ```
fn build_loki_payload(
    config: &PluginConfig,
    data: &CapturedData,
) -> (Vec<(String, String)>, Vec<u8>) {
    let mut stream_labels: HashMap<&str, &str> = HashMap::new();
    stream_labels.insert("service", &config.service_name);
    stream_labels.insert("source", "wasm-wiretap");

    // Merge user-supplied labels.
    for (k, v) in &config.labels {
        stream_labels.insert(k.as_str(), v.as_str());
    }

    // Build a summary message for the _msg field (required by VictoriaLogs).
    let msg = format!(
        "{} {} {} → {}",
        data.method, data.path, data.authority, data.status_code
    );

    // Serialize CapturedData to a JSON map, then inject _msg at the top
    // level alongside all the existing fields (no nesting).
    let mut log_map = serde_json::to_value(data)
        .and_then(|v| match v {
            serde_json::Value::Object(m) => Ok(m),
            _ => Ok(serde_json::Map::new()),
        })
        .unwrap_or_default();
    log_map.insert("_msg".to_string(), serde_json::Value::String(msg));

    let log_line = serde_json::to_string(&log_map).unwrap_or_default();
    let ts = format!("{}", data.timestamp_ns);

    // Loki wants a map for "stream" and array-of-arrays for "values".
    let payload = serde_json::json!({
        "streams": [{
            "stream": stream_labels,
            "values": [
                [ ts, log_line ]
            ]
        }]
    });

    let body = serde_json::to_vec(&payload).unwrap_or_default();

    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), "wasm-wiretap/0.1".to_string()),
    ];

    (headers, body)
}

// ─────────────────────── OTLP/HTTP logs payload ───────────────────────

/// Build an OTLP/HTTP JSON log export payload (v1).
///
/// Reference: <https://opentelemetry.io/docs/specs/otlp/#otlphttp>
fn build_otlp_payload(
    config: &PluginConfig,
    data: &CapturedData,
) -> (Vec<(String, String)>, Vec<u8>) {
    let mut resource_attrs = vec![
        otel_kv("service.name", &config.service_name),
        otel_kv("telemetry.sdk.name", "wasm-wiretap"),
        otel_kv("telemetry.sdk.language", "rust"),
    ];

    for (k, v) in &config.labels {
        resource_attrs.push(otel_kv(k, v));
    }

    let log_body = serde_json::to_string(data).unwrap_or_default();

    let severity_number = if data.status_code >= 500 {
        17 // ERROR
    } else if data.status_code >= 400 {
        13 // WARN
    } else {
        9 // INFO
    };

    let severity_text = if data.status_code >= 500 {
        "ERROR"
    } else if data.status_code >= 400 {
        "WARN"
    } else {
        "INFO"
    };

    let payload = serde_json::json!({
        "resourceLogs": [{
            "resource": {
                "attributes": resource_attrs
            },
            "scopeLogs": [{
                "scope": {
                    "name": "wasm-wiretap",
                    "version": "0.1.0"
                },
                "logRecords": [{
                    "timeUnixNano": format!("{}", data.timestamp_ns),
                    "severityNumber": severity_number,
                    "severityText": severity_text,
                    "body": {
                        "stringValue": log_body
                    },
                    "attributes": build_otel_log_attributes(data),
                    "traceId": "",
                    "spanId": ""
                }]
            }]
        }]
    });

    let body = serde_json::to_vec(&payload).unwrap_or_default();

    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), "wasm-wiretap/0.1".to_string()),
    ];

    (headers, body)
}

// ───────────────────────────── Helpers ─────────────────────────────────

fn otel_kv(key: &str, value: &str) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "value": { "stringValue": value }
    })
}

fn build_otel_log_attributes(data: &CapturedData) -> Vec<serde_json::Value> {
    let mut attrs = vec![
        otel_kv("http.method", &data.method),
        otel_kv("http.target", &data.path),
        otel_kv("http.host", &data.authority),
        otel_kv("http.status_code", &data.status_code.to_string()),
        otel_kv("http.duration_ms", &data.duration_ms.to_string()),
    ];

    if let Some(ref rid) = data.request_id {
        attrs.push(otel_kv("http.request_id", rid));
    }
    if let Some(ref src) = data.source_address {
        attrs.push(otel_kv("net.peer.ip", src));
    }
    if let Some(ref dst) = data.destination_address {
        attrs.push(otel_kv("net.host.ip", dst));
    }

    attrs
}
