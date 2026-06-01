# SuperCell build/test entry points.
#
# Canonical local validation targets:
# - `make check`     -> `make dev-check` for incremental source-mounted static checks.
# - `make gate-fast` -> `make dev-gate-fast` for fast source-mounted checks plus tests.
# - `make gate`      -> `make image-gate` for full image-backed validation.
#
# Target families:
# - host-*  run directly on the host or inside the CI toolchain image.
# - image-* run against project builder/runtime images without source mounts.
# - dev-*   run inside the builder image with source and cache volumes mounted.

CMD_BUILD := cargo build --workspace
CMD_TEST := cargo build --workspace && cargo test --workspace
CMD_TEST_FAST := cargo test --workspace --lib --bins
CMD_CHECK_FAST := cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
CMD_DOC := cargo doc --workspace --no-deps --document-private-items
CMD_CHECK := $(CMD_CHECK_FAST) && $(CMD_DOC)
CMD_FMT := cargo fmt --all
CMD_AUDIT := cargo audit
CMD_DENY := cargo deny check --config .cargo/deny.toml
CMD_LICENSES := cargo about generate --config .cargo/about.toml .cargo/about.hbs > THIRD_PARTY_LICENSES.html

# ---- Canonical validation aliases ----
.PHONY: gate gate-fast build build-no-cache clean build-builder \
	test test-fast check check-fast fmt audit doc dev-doc host-doc dev-licenses host-licenses

doc:
	$(MAKE) dev-doc

host-doc:
	$(CMD_DOC)
	mkdir -p docs && rm -rf docs/html && cp -r target/doc docs/html

dev-doc: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c '$(CMD_DOC) && mkdir -p /workspace/docs && rm -rf /workspace/docs/html && cp -r /workspace/target/doc /workspace/docs/html'

gate:
	$(MAKE) image-gate

gate-fast:
	$(MAKE) dev-gate-fast

build:
	$(MAKE) image-build

build-no-cache:
	$(MAKE) image-build-no-cache

clean:
	$(MAKE) image-clean
	$(MAKE) host-clean

build-builder:
	$(MAKE) image-build-builder

test:
	$(MAKE) image-test

test-fast:
	$(MAKE) dev-test-fast

check:
	$(MAKE) dev-check

check-fast:
	$(MAKE) dev-check-fast

fmt:
	$(MAKE) dev-fmt

audit:
	$(MAKE) image-audit

# ---- Host (no container, used by CI) ----
.PHONY: host-gate host-gate-fast host-build host-test host-test-fast \
	host-check host-check-fast host-fmt host-audit host-licenses host-clean

host-gate: host-test host-check
host-gate-fast: host-test-fast host-check-fast

host-build:
	$(CMD_BUILD)

host-test:
	$(CMD_TEST)

host-test-fast:
	$(CMD_TEST_FAST)

host-check:
	$(CMD_CHECK)

host-check-fast:
	$(CMD_CHECK_FAST)

host-fmt:
	$(CMD_FMT)

host-audit:
	@test -f LICENSE || test -f LICENSE.md || { echo "ERROR: LICENSE or LICENSE.md must exist at repository root"; exit 1; }
	@test -f INTENT.md || { echo "ERROR: INTENT.md must exist at repository root"; exit 1; }
	@test -f CONTRIBUTING.md || { echo "ERROR: CONTRIBUTING.md must exist at repository root"; exit 1; }
	@command -v cargo-audit >/dev/null 2>&1 || { echo "ERROR: cargo-audit is required; use the builder image or install the pinned toolchain"; exit 1; }
	@command -v cargo-deny  >/dev/null 2>&1 || { echo "ERROR: cargo-deny is required; use the builder image or install the pinned toolchain"; exit 1; }
	$(CMD_AUDIT)
	$(CMD_DENY)

host-licenses:
	@command -v cargo-about >/dev/null 2>&1 || { echo "ERROR: cargo-about is required; use the builder image or install the pinned toolchain"; exit 1; }
	$(CMD_LICENSES)

host-clean:
	rm -rf target dist build .cache tmp docs/html
	rm -f THIRD_PARTY_LICENSES.html sbom-*.cdx.json

# ---- Container Runtime ----
# Resolved lazily; host-* targets don't need a container runtime.
CONTAINER_RT := $(shell command -v podman 2>/dev/null || command -v docker 2>/dev/null)
COMPOSE ?= podman-compose

# ---- Image Tagging ----
IMAGE_NAME ?= registry.gitlab.com/open-arsenal/ams-gra/hello-world-sk/sim/supercell

ifdef CI
COMMIT_SHA := $(shell git rev-parse --short HEAD)
BRANCH := $(shell git rev-parse --abbrev-ref HEAD | sed 's/[^a-zA-Z0-9._-]/-/g')
VERSION := $(shell git describe --tags --abbrev=0 2>/dev/null || echo "dev")
TAG := $(COMMIT_SHA)
else
TAG := latest
endif

RUNTIME_IMG := $(IMAGE_NAME):$(TAG)
BUILDER_IMG := $(IMAGE_NAME)/builder:$(TAG)

# Persistent local caches for faster incremental rebuilds.
CACHE_KEY ?= supercell
CARGO_REGISTRY_VOL := $(CACHE_KEY)-cargo-registry
CARGO_GIT_VOL := $(CACHE_KEY)-cargo-git
TARGET_VOL := $(CACHE_KEY)-target
IMAGE_RUN_ARGS := --rm -w /build
DEV_RUN_ARGS := --rm \
	-v $(CURDIR):/workspace:Z \
	-v $(CARGO_REGISTRY_VOL):/usr/local/cargo/registry \
	-v $(CARGO_GIT_VOL):/usr/local/cargo/git \
	-v $(TARGET_VOL):/workspace/target \
	-w /workspace

# ---- Image-backed targets (no source mounts) ----
.PHONY: image-gate image-build image-build-no-cache image-clean \
	image-build-builder image-test image-check image-audit \
	compose-up compose-build-up compose-dev-up compose-metrics-up \
	compose-metrics-down

image-gate: image-test image-check image-build

image-build:
	$(CONTAINER_RT) build --target runtime -t $(RUNTIME_IMG) -f Containerfile .
ifdef CI
	$(CONTAINER_RT) tag $(RUNTIME_IMG) $(IMAGE_NAME):$(BRANCH)
	$(CONTAINER_RT) tag $(RUNTIME_IMG) $(IMAGE_NAME):$(VERSION)
	@if [ "$(BRANCH)" = "main" ]; then $(CONTAINER_RT) tag $(RUNTIME_IMG) $(IMAGE_NAME):latest; fi
endif

image-build-no-cache:
	$(CONTAINER_RT) build --no-cache --target runtime -t $(RUNTIME_IMG) -f Containerfile .

image-clean:
	$(CONTAINER_RT) images --filter "reference=$(IMAGE_NAME)" -q | sort -u | xargs -r $(CONTAINER_RT) rmi -f
	$(CONTAINER_RT) images --filter "reference=$(IMAGE_NAME)/builder" -q | sort -u | xargs -r $(CONTAINER_RT) rmi -f

image-build-builder:
	$(CONTAINER_RT) build --target builder -t $(BUILDER_IMG) -f Containerfile .

image-test: image-build-builder
	$(CONTAINER_RT) run $(IMAGE_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_TEST)"

image-check: image-build-builder
	$(CONTAINER_RT) run $(IMAGE_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_CHECK)"

image-audit: image-build-builder
	$(CONTAINER_RT) run $(IMAGE_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_AUDIT) && $(CMD_DENY)"

compose-up:
	$(COMPOSE) up

compose-build-up:
	$(COMPOSE) -f compose.yaml -f compose.build.yaml up --build

compose-dev-up:
	$(COMPOSE) -f compose.yaml -f compose.dev.yaml up --build

compose-metrics-up:
	$(COMPOSE) -f compose.metrics.yaml up

compose-metrics-down:
	$(COMPOSE) -f compose.metrics.yaml down

# ---- Development targets (source-mounted local iteration) ----
.PHONY: dev dev-gate dev-gate-fast dev-build dev-test dev-test-fast \
	dev-check dev-check-fast dev-fmt dev-audit dev-licenses

dev-gate: dev-test dev-check
dev-gate-fast: dev-test-fast dev-check-fast

dev: image-build-builder
	$(CONTAINER_RT) run --rm -it \
		-v $(CURDIR):/workspace:Z \
		-v $(CARGO_REGISTRY_VOL):/usr/local/cargo/registry \
		-v $(CARGO_GIT_VOL):/usr/local/cargo/git \
		-v $(TARGET_VOL):/workspace/target \
		-w /workspace \
		$(BUILDER_IMG) $(ARGS)

dev-build: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_BUILD)"

dev-test: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_TEST)"

dev-test-fast: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_TEST_FAST)"

dev-check: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_CHECK)"

dev-check-fast: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_CHECK_FAST)"

dev-fmt: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_FMT)"

dev-audit: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_AUDIT) && $(CMD_DENY)"

dev-licenses: image-build-builder
	$(CONTAINER_RT) run $(DEV_RUN_ARGS) $(BUILDER_IMG) sh -c "$(CMD_LICENSES)"
