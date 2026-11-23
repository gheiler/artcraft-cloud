# syntax=docker/dockerfile:1
# artcraftCloud: one image, one process, one volume.
# Build:  docker build -t artcraft-cloud .
# Run:    docker run -d -p 8080:8080 -v artcraft-data:/data artcraft-cloud

ARG RUST_IMAGE=docker.io/library/rust:1-slim-bookworm
ARG RUNTIME_IMAGE=docker.io/library/debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
ARG TRUNK_VERSION=0.22.2
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} CARGO_TERM_COLOR=never
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add wasm32-unknown-unknown \
    && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-musl.tar.gz" \
       | tar -xz -C /usr/local/bin trunk
WORKDIR /src
COPY . .
# The toolchain file asks for "stable"; use the image's toolchain instead of downloading another.
RUN rm -f rust-toolchain.toml \
    && cargo build --release --locked -p artcraft-cloud \
    && cd crates/shell && trunk build --release

FROM ${RUNTIME_IMAGE}
# gpg verifies the signed SHA256SUMS that ArtCraft releases publish; tini reaps and forwards signals.
RUN apt-get update && apt-get install -y --no-install-recommends gpg gpg-agent tini ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin artcraft \
    && mkdir -p /data && chown artcraft /data
COPY --from=build /src/target/release/artcraft-cloud /usr/local/bin/artcraft-cloud
COPY --from=build /src/crates/shell/dist /opt/artcraft-cloud/shell
COPY scripts/update-apps.sh /usr/local/bin/update-apps
ENV ARTCRAFT_DATA=/data \
    ARTCRAFT_LISTEN=0.0.0.0:8080 \
    ARTCRAFT_SHELL_DIR=/opt/artcraft-cloud/shell \
    TOKIO_WORKER_THREADS=2 \
    MALLOC_ARENA_MAX=2 \
    MALLOC_MMAP_THRESHOLD_=131072 \
    MALLOC_TRIM_THRESHOLD_=131072
USER artcraft
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["artcraft-cloud", "healthcheck"]
ENTRYPOINT ["/usr/bin/tini", "--", "artcraft-cloud"]
CMD ["serve"]
