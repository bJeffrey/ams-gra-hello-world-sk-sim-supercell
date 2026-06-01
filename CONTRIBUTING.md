# Contributing to SuperCell

## Prerequisites

- [Podman](https://podman.io/getting-started/installation) (or Docker)
- [podman-compose](https://github.com/containers/podman-compose) (`pip install podman-compose`)
- Git

For native development without containers, you also need:
- Rust toolchain (see `rust-toolchain.toml` for the pinned version)
- `cargo-audit`, `cargo-deny`, `cargo-about` (installed automatically by `make native-audit` / `make native-licenses`)

## Building and testing locally

All local development uses containerized `dev-*` Make targets. The first run builds the builder image; subsequent runs reuse cached layers.

```bash
# Build
make dev-build

# Run tests (full)
make dev-test

# Run tests (fast — lib + bin targets only, skips doc tests)
make dev-test-fast

# Lint and format check
make dev-check-fast      # fmt + clippy
make dev-check           # fmt + clippy + doc build

# Security and license audit
make dev-audit
```

### Running the full stack locally

```bash
# Run the published images
podman-compose up -d

# Local source build
make compose-build-up
```

The default scenario (`config/default.toml`) runs 2 aircraft — 1 blue ownship (Eagle-1) and 1 red bandit (Bandit-1) — flying a pentagon pattern in Ohio at 4000/4200 ft MSL.

## Merge request requirements

Before opening an MR, ensure:

1. **Tests pass**: `make dev-test`
2. **Lints pass**: `make dev-check`
3. **No new warnings**: clippy must be clean (`-D warnings` is enforced)
4. **Tests cover the change**: new logic needs tests that verify behavior, not implementation details. See the test guidelines below.
5. **Commit messages are clear**: describe *what* changed and *why*, not *how*.

The CI pipeline runs `check → test → audit → build` on every MR. All stages must pass before merge.

## CI workflow summary

SuperCell CI is split by pipeline type:

- **Feature/fix branches (`feat/*`, `fix/*`)**
  - Run build + test path for fast feedback (`build-builder`, `check`, `test`, image builds).
  - `audit` and `sbom` are skipped on these branches.
- **Merge requests / `main`**
  - Run full validation including `audit` and `sbom`.
- **Release tags (`v*`)**
  - Do not rebuild images from source.
  - CI promotes already-built immutable SHA images (`:<sha>`) to release tags (`:<tag>`), then bundles and publishes release artifacts.

To reduce duplicate pipelines, branch pipelines are skipped when an MR is open (`CI_OPEN_MERGE_REQUESTS`).

### Test guidelines

- Test **behavior**, not implementation details
- Use **inline fixtures** (e.g., inline TOML strings), not external files
- Do not grep source code, logs, or output as assertions
- Do not test file existence or project structure
- One logical concept per test with a descriptive name
- Mocks are acceptable for unit tests of application logic; integration tests should use real interfaces

## Project structure

| Path | Purpose |
|---|---|
| `src/` | Rust source |
| `tests/` | Integration tests |
| `config/` | Default scenario TOML and JSBSim aircraft definitions |
| `jsbsim/` | JSBSim Containerfile (builds the dynamics engine from source) |
| `scripts/` | `bundle.sh` (release packaging) and utility scripts |
| `LICENSES/` | Third-party license files for non-crate dependencies |
| `.cargo/` | `deny.toml`, `about.toml`, `about.hbs` (audit and license report configs) |

## Release process

Releases are triggered by pushing a calver tag:

```bash
git tag v2026.03.25
git push origin v2026.03.25
```

The CI release pipeline then:

1. **Validates** — runs check, test, audit (including license checks)
2. **Builds** three container images:
   - `supercell:<tag>` — runtime image (AlmaLinux 10-micro)
   - `supercell/builder:<tag>` — full build toolchain for air-gapped rebuilds
   - `supercell/jsbsim:<tag>` — JSBSim flight dynamics engine
3. **Bundles** two tar artifacts:
   - `supercell-<tag>.tar.gz` (runtime) — container images, compose file, config, licenses
   - `supercell-<tag>-devkit.tar.gz` — adds builder image and source archive
4. **Publishes** both tars to the GitLab Generic Package Registry
5. **Creates** a GitLab Release with download links

### Tag format

- `v2026.03.25` — stable release
- `v2026.03.25-alpha.1` — pre-release
- `v2026.03.25-rc.1` — release candidate
- `v2026.03.25-hotfix.1` — same-day fix after a stable release

### What goes in the bundle

Each release tar is self-contained and includes:

| File | Purpose |
|---|---|
| `images/*.tar` | Container images (`podman load` ready) |
| `compose.yaml` | Service definitions for `podman-compose up` |
| `env.example` | Overridable environment variables |
| `config/` | Default scenario and JSBSim aircraft configs |
| `jsbsim/` | JSBSim Containerfile and build recipe (LGPL compliance) |
| `LICENSE` | Project license (Apache 2.0) |
| `LICENSES/` | Third-party dependency licenses (non-crate) |
| `THIRD_PARTY_LICENSES.html` | Auto-generated license report for all Rust crate dependencies |
| `manifest.json` | Provenance metadata (tag, commit SHA, pipeline URL, dependency list) |
| `SHA256SUMS` | Integrity verification for all files in the bundle |

The devkit variant additionally includes `src/` (source archive), `compose.dev.yaml` (builder overlay), and the builder container image.

## Licensing

SuperCell is licensed under Apache 2.0. See `LICENSE`.

Third-party dependencies fall into two categories:

- **Rust crates** — audited by `cargo deny` in CI, license report generated by `cargo-about`
- **Non-crate dependencies** (JSBSim) — tracked manually in `LICENSES/` with machine-readable `LICENSE:` and `SOURCE:` headers in each `README`

When adding a new non-crate dependency:

1. Create `LICENSES/<name>/LICENSE` with the dependency's license text
2. Create `LICENSES/<name>/README` with headers:
   ```
   LICENSE: <SPDX-identifier>
   SOURCE: <URL-to-upstream-license-at-pinned-version>
   ```
3. Add a `dependencies` entry in `scripts/bundle.sh`'s manifest.json generation
4. If the dependency is LGPL, ensure the build recipe is included in both runtime and devkit tars
