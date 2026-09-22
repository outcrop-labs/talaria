- **`box start` now converges with `up -d`** (was `compose start`, which merely re-launches the
  existing containers): the documented stop → edit `compose.override.yml` → start flow for
  auth/env changes silently never applied. Regression-tested against the verb.

### Documentation
