---
LICENSE: LGPL-2.1-only
SOURCE: https://raw.githubusercontent.com/JSBSim-Team/jsbsim/v1.2.4/COPYING
---

# JSBSim Flight Dynamics Model

- **Repository:** https://github.com/JSBSim-Team/jsbsim
- **Version:** 1.2.4
- **License:** LGPL-2.1-only (see LICENSE in this directory)

## Modifications

The following patch is applied at build time in `jsbsim/Containerfile`:

```
sed -i 's/setprecision(6)/setprecision(15)/g' src/input_output/FGInputSocket.cpp
```

This increases the TCP console output precision from 6 to 15 significant
digits. The stock `setprecision(6)` quantizes geodetic position to ~50m;
15 gives sub-millimeter resolution required for DIS entity state reporting.

## Build recipe

See `jsbsim/Containerfile` for the complete build from source, including the patch above.
