FROM rust:1.93.1-trixie AS builder

RUN rustup target add wasm32-wasip1

WORKDIR /build
COPY . .
RUN cargo build --target wasm32-wasip1 --release

# ── Output stage: just the .wasm binary ───────────────────────────────
FROM scratch
COPY --from=builder /build/target/wasm32-wasip1/release/wasm_wiretap.wasm /plugin.wasm
