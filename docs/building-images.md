# Building Container Images

If you do not have access to the remote registry (`registry.gitlab.com`) or prefer to compile and build the project from source, you can build the container images locally before starting the environment.

## 1. Resolve Dependencies

`supercell` relies on the `sleet` crates. You must clone the `sleet` repository into the `supercell` directory so the Rust compiler can find the local paths specified in `Cargo.toml`.

From the `sim/supercell` directory, run:

```bash
git clone https://gitlab.com/open-arsenal/ams-gra/hello-world-sk/infra/sleet sleet
```

*(Note: Ensure you add `sleet/` to your local `.git/info/exclude` if you plan to make local commits to `supercell`, as nested git repositories can cause tracking issues).*

## 2. Build the Images

Run the following command from the root of the project to build all required images:

```bash
podman-compose build
```

## 3. Start the Environment

Once the build process completes and the images are available locally, you can start the environment normally:

```bash
podman-compose up -d
```
