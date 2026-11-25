# Build time, and where the apps come from

> **Last updated:** 2026-11-24

## What was slow

Three separate things looked like "it downloads everything again":

1. **The image build redid everything on any change.** The old Dockerfile copied the whole tree
   and then ran `cargo build`, so changing one line meant downloading every crate again (about 220)
   and compiling every crate again (about 370). On every build, trunk also downloaded wasm-bindgen
   and wasm-opt.
2. **The apps were downloaded again for every new data volume.** The ArtCraft apps (nine web builds,
   about 200 MB of archives and 600 MB unpacked) install into the data volume on first start.
   `docker compose up` makes a volume named `<project>_artcraft-data` (for example
   `artcraft-cloud_artcraft-data`). `docker run -v artcraft-data:/data` makes a different one
   called `artcraft-data`. So does another project directory, or a `down -v`. Each new, empty
   volume downloaded all nine apps again. A volume that already has them never downloads them
   again; `apps update` only fetches a newer release.
3. **CI** builds the image with `ARTCRAFT_SKIP_APP_SYNC=1` and never downloads the apps. Its
   slow part was the Rust and trunk build.

## What the build does now

The `Dockerfile` has three layers, ordered from the one that changes least to the one that
changes most:

| Layer | Rebuilt when | What it holds |
|---|---|---|
| `toolchain` | the base image or a pinned tool changes | trunk 0.22.2, wasm-bindgen 0.2.130, wasm-opt (binaryen 131) and cargo-chef 0.1.78. Each is downloaded once and checked against its SHA-256 in the Dockerfile. |
| deps (`cargo chef cook`) | `Cargo.lock` or a `Cargo.toml` changes | Every dependency compiled, for the server and for the wasm shell. The crate registry is a BuildKit cache mount, so even this layer downloads only crates it doesn't have yet. |
| `build` | any source change | Only artcraftCloud's own crates, plus `trunk build --release --offline`. |

- `crates/shell/Trunk.toml` pins the same wasm-bindgen and wasm-opt versions. That is how trunk
  finds them in the image and never downloads anything (`--offline` makes sure of it).
- `rust-toolchain.toml` is in `.dockerignore`, so rustup doesn't fetch a second toolchain inside
  the image.
- In CI, `docker/build-push-action` keeps using the GitHub Actions layer cache
  (`cache-from/to: type=gha, mode=max`). The deps layer is an ordinary layer, so it is cached there
  too. (A cache mount on its own would not be: it never leaves the runner.)
- The `rust` job caches `~/.cache/trunk` (wasm-bindgen and wasm-opt), keyed on `Trunk.toml`. It
  also checks trunk's SHA-256 before using it. `Swatinem/rust-cache` already caches cargo.

### Measured (podman 5.4, 8 cores, `CARGO_BUILD_JOBS=4`)

| Build | Before | After |
|---|---|---|
| Cold (`--no-cache`, no cache mounts) | 190 s | 184 s |
| Warm, one source file changed | 150 s: about 220 crates downloaded, about 370 compiled, trunk tools downloaded | **41 s**: nothing downloaded, 5 crates compiled |

- A cold build costs about the same as before: the work has to be done once.
- Every rebuild after that pays only for the layer that changed.
- A change to `Cargo.lock` reruns the deps layer, but it downloads only crates that are new.

## Why the apps are not baked into the image

- **Size.** The image is about 106 MB. With the apps in it, it would be about 700 MB, and every
  update of any one app would mean a new image to build, push and pull.
- **Update cycles.** The apps release on their own schedule. The super admin's Apps page (or
  `artcraft-cloud apps update`) installs a new version into the volume without restarting, once the
  signed [app registry](registry.md) allows it; it verifies the pinned SHA-256 and the publisher's
  signature, and keeps the previous version for a one-step rollback. Baking the
  apps in would tie each app update to an artcraftCloud release, and that rollback would be lost.
- **CI.** CI doesn't need the apps at all, and it would have to download 200 MB on each change to
  the catalog.

Instead, the downloaded archives are kept in an **app cache** that every data volume on the host
shares:

- `ARTCRAFT_APP_CACHE=/cache` with a volume named `artcraft-app-cache`. Compose sets both, and
  the `docker run` line in the README does the same.
- The volume has a fixed `name:`, so the compose project name doesn't change it.
- A fresh data volume installs from the cache. The release's small signed `SHA256SUMS.txt` (and
  its signature) is still fetched and checked. An archive is taken from the cache only when its
  SHA-256 matches that list, so the cache needs no trust of its own. A wrong or partial file is
  downloaded again.
- The cache keeps the current archive and the one before it for each app (about 200 to 400 MB).
- Deleting it is safe: `docker volume rm artcraft-app-cache`. `docker compose down -v` deletes it
  too.

| Installing all nine apps into a new data volume | Downloaded | Time (box) |
|---|---|---|
| Without the cache | about 199 MB | 84 s |
| From the cache | the checksum lists and signatures only (KB) | 40 s (mostly unpacking 600 MB) |

On a slower connection, the saving is the 199 MB download.

- **Kubernetes.** The Helm chart and the manifest keep one PersistentVolumeClaim, which survives
  restarts and upgrades. Leave `ARTCRAFT_APP_CACHE` unset there unless several releases share a
  node volume.
