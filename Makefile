# cecris build and test helpers

CARGO   ?= cargo
NODE    ?= node
BIN     ?= target/release/cecris
E2E_LOG ?= e2e-server.log

.DEFAULT_GOAL := help
.PHONY: help build release run dev test e2e maintenance check fmt lint clean

help:
	@echo "cecris"
	@echo ""
	@echo "  make build    debug build"
	@echo "  make release  optimized binary ($(BIN))"
	@echo "  make run      build and serve on 127.0.0.1:8099"
	@echo "  make dev      cargo run --release"
	@echo "  make test     cargo test"
	@echo "  make e2e      release build, start server, run protocol e2e tests"
	@echo "  make maintenance  graceful SIGTERM shutdown test"
	@echo "  make check    test + e2e + maintenance"
	@echo "  make fmt      cargo fmt"
	@echo "  make lint     cargo clippy -D warnings"
	@echo "  make clean    remove build output and e2e log"
	@echo ""
	@echo "  address and port are binary options:"
	@echo "    $(BIN) --bind 0.0.0.0 --port 8080"

build:
	$(CARGO) build

release:
	$(CARGO) build --release

run: release
	$(BIN) --bind 127.0.0.1 --port 8099

dev:
	$(CARGO) run --release -- --bind 127.0.0.1 --port 8099

test:
	$(CARGO) test

e2e: release
	@set -e; \
	if curl -fsS http://127.0.0.1:8099/healthz >/dev/null 2>&1; then \
		echo "something already serves 127.0.0.1:8099; stop it first" >&2; exit 1; \
	fi; \
	$(BIN) --bind 127.0.0.1 --port 8099 >$(E2E_LOG) 2>&1 & \
	pid=$$!; \
	trap 'kill $$pid 2>/dev/null || true' EXIT INT TERM; \
	i=0; ready=; \
	while [ $$i -lt 50 ]; do \
		if curl -fsS http://127.0.0.1:8099/healthz >/dev/null 2>&1; then ready=1; break; fi; \
		i=$$((i + 1)); sleep 0.1; \
	done; \
	if [ -z "$$ready" ]; then echo "server failed to start; see $(E2E_LOG)" >&2; exit 1; fi; \
	BASE=http://127.0.0.1:8099 $(NODE) tests/e2e.mjs

maintenance: release
	$(NODE) tests/maintenance.mjs

check: test e2e maintenance

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --all-targets -- -D warnings

clean:
	$(CARGO) clean
	rm -f $(E2E_LOG)
