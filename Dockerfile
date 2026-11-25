# syntax=docker/dockerfile:1.7
# artcraftCloud: one image, one process, one volume.
# Build:  docker build -t artcraft-cloud .        (BuildKit; podman/buildah work too)
# Run:    docker run -d -p 8080:8080 -v artcraft-data:/data \
#           -v artcraft-app-cache:/cache -e ARTCRAFT_APP_CACHE=/cache artcraft-cloud
#
# Layers, from the most to the least stable, so a rebuild redoes only what changed:
#   toolchain  pinned trunk, wasm-bindgen, wasm-opt and cargo-chef (checksums below); trunk
#              then builds --offline and never downloads tools during the build
#   deps       every crate compiled once (cargo-chef), keyed on Cargo.lock and the manifests
#   build      only artcraftCloud's own crates
# The ArtCraft apps are not in the image: they install, signature-checked, into the data volume
# on first start (docs/build.md explains why and how a cache avoids downloading them again).

ARG RUST_IMAGE=docker.io/library/rust:1-slim-bookworm
ARG RUNTIME_IMAGE=docker.io/library/debian:bookworm-slim

FROM ${RUST_IMAGE} AS toolchain
ARG TRUNK_VERSION=0.22.2
ARG TRUNK_SHA256=4a90216d93c553f3d8b7401fce0dee588e0fb1eb1c6a379ebac81fabe820f25b
# Must match wasm-bindgen in Cargo.lock and [tools] in crates/shell/Trunk.toml.
ARG WASM_BINDGEN_VERSION=0.2.130
ARG WASM_BINDGEN_SHA256=9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f
ARG BINARYEN_VERSION=131
ARG BINARYEN_SHA256=b5bf1f0eaf17c63ee588ff7a5954dc8f6ce2c26989051c66f24dfe9ece3e46db
ARG CHEF_VERSION=0.1.78
ARG CHEF_SHA256=aca691abfbfbbe00d482e0ed2249eec3091b65e96a0ce92947fb5b254d48b16d
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates xz-utils \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add wasm32-unknown-unknown \
    && fetch() { curl -fsSL "$1" -o /tmp/dl && echo "$2  /tmp/dl" | sha256sum -c - >/dev/null; } \
    && fetch "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-musl.tar.gz" "$TRUNK_SHA256" \
    && tar -xzf /tmp/dl -C /usr/local/bin trunk \
    && fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" "$WASM_BINDGEN_SHA256" \
    && tar -xzf /tmp/dl -C /usr/local/bin --strip-components=1 "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen" \
    && fetch "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/binaryen-version_${BINARYEN_VERSION}-x86_64-linux.tar.gz" "$BINARYEN_SHA256" \
    && tar -xzf /tmp/dl -C /usr/local/bin --strip-components=2 "binaryen-version_${BINARYEN_VERSION}/bin/wasm-opt" \
    && fetch "https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CHEF_VERSION}/cargo-chef-x86_64-unknown-linux-musl.tar.xz" "$CHEF_SHA256" \
    && tar -xJf /tmp/dl -C /usr/local/cargo/bin --strip-components=1 cargo-chef-x86_64-unknown-linux-musl/cargo-chef \
    && rm /tmp/dl
ENV CARGO_TERM_COLOR=never
WORKDIR /src

# The dependency list only (Cargo.toml files and Cargo.lock; sources do not change it).
FROM toolchain AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM toolchain AS build
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
COPY .cargo .cargo
COPY --from=planner /src/recipe.json recipe.json
# Dependencies, native and wasm. Cached as a layer (also in CI's registry cache) until the recipe
# changes; the cache mount keeps downloaded crates across those changes on one machine.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --release --locked -p artcraft-cloud --recipe-path recipe.json \
    && cargo chef cook --release --locked -p artcraft-cloud-shell --target wasm32-unknown-unknown --recipe-path recipe.json
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --locked -p artcraft-cloud \
    && cd crates/shell && trunk build --release --offline

FROM ${RUNTIME_IMAGE}
# gpg verifies the signed SHA256SUMS that ArtCraft releases publish; tini reaps and forwards signals.
RUN apt-get update && apt-get install -y --no-install-recommends gpg gpg-agent tini ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin artcraft \
    && mkdir -p /data /cache && chown artcraft /data /cache
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
