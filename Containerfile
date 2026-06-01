# All build args — defined once at the top.
# CI overrides BUILDER_PREFIX/BUILDER_TAG to pull cached builder from registry.
ARG BUILD_IMAGE=quay.io/almalinuxorg/almalinux:10.1-20260129
ARG RUNTIME_IMAGE=quay.io/almalinuxorg/10-micro:10.1-20260129
ARG BUILDER_PREFIX=
ARG BUILDER_TAG=

# ---- Dev ----
FROM ${BUILD_IMAGE} AS dev

RUN dnf install -y gcc git make ca-certificates curl pkgconf-pkg-config && dnf clean all

COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN RUST_VERSION=$(grep 'channel' /tmp/rust-toolchain.toml | awk -F'"' '{print $2}') \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain ${RUST_VERSION} \
    && rm -f /tmp/rust-toolchain.toml

ENV PATH="/root/.cargo/bin:${PATH}"

RUN rustup component add rustfmt clippy \
    && cargo install cargo-audit --version 0.22.1 --locked \
    && cargo install cargo-deny --version 0.19.4 --locked \
    && cargo install cargo-about --version 0.9.0 --locked --features cli \
    && command -v cargo-audit \
    && command -v cargo-deny \
    && command -v cargo-about

# ---- Builder ----
FROM dev AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY sleet/ sleet/
COPY src/ src/
COPY tests/ tests/
RUN cargo build --workspace --release \
    && cargo doc --workspace --no-deps --document-private-items

# ---- Builder Cache ----
FROM ${BUILDER_PREFIX}builder${BUILDER_TAG} AS builder-cache

# ---- Runtime ----
FROM ${RUNTIME_IMAGE} AS runtime

# No package manager in micro — create non-root user manually.
RUN echo 'appuser:x:1000:1000::/home/appuser:/bin/sh' >> /etc/passwd && \
    echo 'appuser:x:1000:' >> /etc/group && \
    mkdir -p /home/appuser /config && chown 1000:1000 /home/appuser

COPY --from=builder-cache /build/target/release/supercell /usr/local/bin/supercell
COPY config/default.toml /config/default.toml

USER 1000

ENV SUPERCELL_ADMIN_PORT=21300

HEALTHCHECK --interval=2s --timeout=1s --start-period=30s --retries=3 \
  CMD supercell --health-check || exit 1

ENTRYPOINT ["/usr/local/bin/supercell"]
CMD ["--help"]
