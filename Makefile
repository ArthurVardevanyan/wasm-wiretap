.PHONY: build build-debug clean check clippy fmt size dist image push

REGISTRY := registry.arthurvardevanyan.com/homelab/wasm-wiretap
TAG ?= $(shell date --utc '+"%Y.%m.%d.%H%M%S"'-local)
IMAGE := $(REGISTRY):$(TAG)

WASM_TARGET := wasm32-wasip1
OUT_DIR := target/$(WASM_TARGET)/release
WASM_FILE := $(OUT_DIR)/wasm_wiretap.wasm

# ── Build ──────────────────────────────────────────────────────────────

build:
	cargo build --target $(WASM_TARGET) --release

build-debug:
	cargo build --target $(WASM_TARGET)

# ── Checks ─────────────────────────────────────────────────────────────

check:
	cargo check --target $(WASM_TARGET)

clippy:
	cargo clippy --target $(WASM_TARGET) -- -D warnings

fmt:
	cargo fmt -- --check

# ── Helpers ────────────────────────────────────────────────────────────

clean:
	cargo clean

# Show the size of the compiled .wasm binary.
size: build
	@ls -lh $(WASM_FILE)
	@wc -c < $(WASM_FILE) | awk '{printf "%.2f KiB\n", $$1/1024}'

# Copy the wasm binary to a known location for easy reference.
dist: build
	mkdir -p dist
	cp $(WASM_FILE) dist/wasm-wiretap.wasm
	@echo "→ dist/wasm-wiretap.wasm"

# ── OCI Image ──────────────────────────────────────────────────────────

image:
	podman build -t $(IMAGE) -f Containerfile .
	@echo "→ $(IMAGE)"

push: image
	podman push $(IMAGE)
	@echo "→ pushed $(IMAGE)"
