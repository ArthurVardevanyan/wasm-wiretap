mod capture;
mod config;
mod exporters;

use capture::CapturedData;
use config::PluginConfig;
use log::{debug, error, info, warn};
use proxy_wasm::traits::*;
use proxy_wasm::types::*;
use std::rc::Rc;
use std::time::Duration;

// ─────────────────────────── Entry point ──────────────────────────────

proxy_wasm::main! {{
    proxy_wasm::set_log_level(LogLevel::Info);
    proxy_wasm::set_root_context(|_context_id| -> Box<dyn RootContext> {
        Box::new(WiretapRoot {
            config: Rc::new(PluginConfig::default()),
        })
    });
}}

// ────────────────────────── Root Context ──────────────────────────────

struct WiretapRoot {
    config: Rc<PluginConfig>,
}

impl Context for WiretapRoot {}

impl RootContext for WiretapRoot {
    fn on_configure(&mut self, _plugin_configuration_size: usize) -> bool {
        let config_bytes = self
            .get_plugin_configuration()
            .unwrap_or_default();

        match PluginConfig::from_bytes(&config_bytes) {
            Ok(cfg) => {
                info!(
                    "wasm-wiretap: configured – backend={:?}, upstream={}",
                    cfg.backend,
                    cfg.upstream_cluster,
                );
                self.config = Rc::new(cfg);
                true
            }
            Err(e) => {
                error!("wasm-wiretap: failed to parse config: {}", e);
                false
            }
        }
    }

    fn create_http_context(&self, _context_id: u32) -> Option<Box<dyn HttpContext>> {
        Some(Box::new(WiretapHttp {
            config: Rc::clone(&self.config),
            data: CapturedData::new(),
            request_body_buffer: Vec::new(),
            response_body_buffer: Vec::new(),
        }))
    }

    fn get_type(&self) -> Option<ContextType> {
        Some(ContextType::HttpContext)
    }
}

// ────────────────────────── HTTP Context ──────────────────────────────

struct WiretapHttp {
    config: Rc<PluginConfig>,
    data: CapturedData,
    request_body_buffer: Vec<u8>,
    response_body_buffer: Vec<u8>,
}

impl WiretapHttp {
    /// Non-blocking: fire-and-forget export of the captured data.
    /// Uses `dispatch_http_call` which is Envoy's async HTTP mechanism;
    /// it does NOT block the data path.
    fn export_async(&self) {
        if self.config.upstream_cluster.is_empty() {
            warn!("wasm-wiretap: no upstream_cluster configured – skipping export");
            return;
        }

        let (headers, body) = exporters::build_export_payload(&self.config, &self.data);

        let path = self.config.upstream_path.clone();
        let authority = self.config.authority();

        let mut h = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), path),
            (":authority".to_string(), authority),
        ];
        h.extend(headers);

        // Convert to &str pairs for the FFI call.
        let header_refs: Vec<(&str, &str)> =
            h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        // dispatch_http_call is asynchronous – it sends the request on
        // a side-channel and invokes `on_http_call_response` when done.
        // The main request/response flow is NOT paused.
        match self.dispatch_http_call(
            &self.config.upstream_cluster,
            header_refs,
            Some(&body),
            vec![],
            Duration::from_secs(5),
        ) {
            Ok(_) => debug!("wasm-wiretap: export dispatched"),
            Err(e) => error!("wasm-wiretap: dispatch_http_call failed: {:?}", e),
        }
    }
}

impl Context for WiretapHttp {
    /// Called when the async export call completes (or times out).
    /// We only log the result; the main traffic flow is already done.
    fn on_http_call_response(
        &mut self,
        _token_id: u32,
        _num_headers: usize,
        body_size: usize,
        _num_trailers: usize,
    ) {
        if let Some(body) = self.get_http_call_response_body(0, body_size) {
            let status = self
                .get_http_call_response_header(":status")
                .unwrap_or_default();
            if status.starts_with('2') {
                debug!("wasm-wiretap: export succeeded ({})", status);
            } else {
                let resp = String::from_utf8_lossy(&body);
                warn!(
                    "wasm-wiretap: export returned status={} body={}",
                    status,
                    &resp[..resp.len().min(256)]
                );
            }
        }
    }
}

impl HttpContext for WiretapHttp {
    // ─── Request phase ─────────────────────────────────────────────

    fn on_http_request_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        // Grab timestamp from Envoy (nanoseconds since epoch).
        let now = self
            .get_property(vec!["request", "time"])
            .and_then(|b| {
                if b.len() >= 8 {
                    Some(u64::from_le_bytes(b[..8].try_into().unwrap()))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        self.data.timestamp_ns = if now > 0 {
            now
        } else {
            // Fallback: use current_time from the proxy-wasm host.
            self.get_current_time()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
        };

        // Basic request metadata.
        self.data.method = self
            .get_http_request_header(":method")
            .unwrap_or_default();
        self.data.path = self
            .get_http_request_header(":path")
            .unwrap_or_default();
        self.data.authority = self
            .get_http_request_header(":authority")
            .unwrap_or_default();
        self.data.request_id = self.get_http_request_header("x-request-id");

        // Source / destination from Envoy properties.
        self.data.source_address = self
            .get_property(vec!["source", "address"])
            .map(|b| String::from_utf8_lossy(&b).to_string());
        self.data.destination_address = self
            .get_property(vec!["destination", "address"])
            .map(|b| String::from_utf8_lossy(&b).to_string());

        // Capture all request headers.
        if self.config.capture_request_headers {
            self.data.request_headers = Some(self.get_http_request_headers());
        }

        Action::Continue
    }

    fn on_http_request_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        if !self.config.capture_request_body {
            return Action::Continue;
        }

        // Skip the (expensive) copy from Envoy once the buffer is full.
        let buffer_full = self.config.max_body_bytes > 0
            && self.request_body_buffer.len() >= self.config.max_body_bytes;

        if !buffer_full {
            // Accumulate body chunks.
            if let Some(chunk) = self.get_http_request_body(0, body_size) {
                let remaining = if self.config.max_body_bytes > 0 {
                    self.config
                        .max_body_bytes
                        .saturating_sub(self.request_body_buffer.len())
                } else {
                    chunk.len()
                };
                self.request_body_buffer
                    .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
        }

        if end_of_stream {
            self.data.request_body =
                Some(String::from_utf8_lossy(&self.request_body_buffer).to_string());
        }

        Action::Continue
    }

    // ─── Response phase ────────────────────────────────────────────

    fn on_http_response_headers(&mut self, _num_headers: usize, _end_of_stream: bool) -> Action {
        self.data.status_code = self
            .get_http_response_header(":status")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        if self.config.capture_response_headers {
            self.data.response_headers = Some(self.get_http_response_headers());
        }

        Action::Continue
    }

    fn on_http_response_body(&mut self, body_size: usize, end_of_stream: bool) -> Action {
        if !self.config.capture_response_body {
            if end_of_stream {
                self.finalise_and_export();
            }
            return Action::Continue;
        }

        // Skip the (expensive) copy from Envoy once the buffer is full.
        let buffer_full = self.config.max_body_bytes > 0
            && self.response_body_buffer.len() >= self.config.max_body_bytes;

        if !buffer_full {
            // Accumulate body chunks.
            if let Some(chunk) = self.get_http_response_body(0, body_size) {
                let remaining = if self.config.max_body_bytes > 0 {
                    self.config
                        .max_body_bytes
                        .saturating_sub(self.response_body_buffer.len())
                } else {
                    chunk.len()
                };
                self.response_body_buffer
                    .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
        }

        if end_of_stream {
            self.data.response_body =
                Some(String::from_utf8_lossy(&self.response_body_buffer).to_string());
            self.finalise_and_export();
        }

        Action::Continue
    }

    fn on_log(&mut self) {
        // Safety net: if on_http_response_body with end_of_stream was
        // never called (e.g. connection reset), we still try to export
        // whatever we have.
        if self.data.response_body.is_none() && self.config.capture_response_body {
            if !self.response_body_buffer.is_empty() {
                self.data.response_body =
                    Some(String::from_utf8_lossy(&self.response_body_buffer).to_string());
            }
        }
        // Final attempt export (may no-op if already exported).
        self.finalise_and_export();
    }
}

impl WiretapHttp {
    fn finalise_and_export(&mut self) {
        // Compute duration.
        if self.data.timestamp_ns > 0 {
            let now = self
                .get_current_time()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            if now > self.data.timestamp_ns {
                self.data.duration_ms = (now - self.data.timestamp_ns) / 1_000_000;
            }
        }

        self.export_async();
    }
}
