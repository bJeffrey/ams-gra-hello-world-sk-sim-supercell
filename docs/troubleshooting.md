# Troubleshooting

This guide provides solutions to common issues encountered when building, integrating, or running supercell.

## Integration & Build Issues

### Dependency Resolution Failures
**Symptoms:** The build system cannot find supercell or its required dependencies.
**Cause:** Incorrect registry configuration, missing credentials, or version mismatches.
**Solution:**
1. Verify you are authenticated with the correct artifact registry.
2. Check the dependency version specified in your build manifest against the available versions.

## Common Runtime Issues

### Initialization Errors
**Symptoms:** The module fails to initialize or crashes on startup.
**Cause:** Invalid configuration or missing environment variables.
**Solution:**
1. Check the logs for specific configuration parsing errors.
2. Ensure all required configuration values and environment variables are provided.

## Getting Help

If you encounter an issue not listed here:
1. Gather the relevant logs, build output, and configuration files.
2. Note the host environment and version being used.
3. Open an issue on the issue tracker with the collected details.
