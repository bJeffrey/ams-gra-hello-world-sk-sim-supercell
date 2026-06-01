# JSBSim Flight Dynamics Container

Builds [JSBSim v1.2.4](https://github.com/JSBSim-Team/jsbsim/tree/v1.2.4) from source with a precision patch and packages it with the stock data tree.

Each container instance runs a single JSBSim process with a TCP console. The port is baked into the aircraft model's `<input port="N"/>` element. The container starts suspended (`--suspend`) — supercell drives it via `iterate()` over TCP.

## Modification

The `setprecision(6)` call in `FGInputSocket.cpp` is patched to `setprecision(15)`. The stock precision quantizes geodetic position to ~50m; 15 significant digits gives sub-millimeter resolution required for DIS entity state reporting.

See `LICENSES/jsbsim/README` for full provenance and LGPL compliance details.

## Usage

```bash
# Build
podman build -t jsbsim:dev -f jsbsim/Containerfile jsbsim/

# Run (aircraft config mounted at runtime)
podman run --rm --network host \
  -v ./config/jsbsim_aircraft/eagle1:/usr/local/jsbsim/aircraft/eagle1:ro \
  jsbsim:dev \
  --aircraft=eagle1 --initfile=reset00
```

*Note: JSBSim starts with `--suspend`, so it will hang indefinitely waiting for an `iterate` command over TCP. You must connect to the aircraft's input port (e.g., `telnet 127.0.0.1 21110` for Eagle-1) to send commands and advance the simulation.*

In production, JSBSim containers are managed by `compose.yaml` — no manual runs needed.

## Signals

JSBSim has no `SIGTERM` handler. The container uses `STOPSIGNAL SIGKILL` to avoid the 10-second shutdown timeout.
