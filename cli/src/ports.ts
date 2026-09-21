// The dev stack's host-port defaults — the `${VAR:-default}` spellings of
// docker/dev-compose.yml, plus the Rust sidecar's loopback port (the one
// ui/.env's TALARIA_RUST_API_URL and the app's proxy default both name). One
// table, so the CLI and compose can never disagree about which port a service
// is on, and a value changed here is changed everywhere it is printed.
//
// Strings, not numbers on purpose: every consumer either prints the value or
// hands it to docker as an argv, and both want the spelling. The runtime
// port SLOTS (worktrees: 53xx/56xx/65xx; boxes: 5301-5389) are allocated by
// paths.ts, not defaults.

/** TALARIA_PG_PORT — dev-compose's `127.0.0.1:<port>:5432`. */
export const PG_PORT = '5544'

/** TALARIA_REDIS_PORT. */
export const REDIS_PORT = '6399'

/** TALARIA_MINIO_PORT — the built-in S3 bucket (Admin → Storage). */
export const MINIO_PORT = '9010'

/** TALARIA_SEARCH_PORT — the SearXNG host port. */
export const SEARCH_PORT = '8888'

/** TALARIA_API_PORT — the Rust api sidecar's in-box loopback bind. */
export const API_PORT = '5274'

/** TALARIA_HTTP_PORT — the app itself: vite in dev, the published container
 *  port in prod. The `ENV PORT` the Dockerfile bakes and the port the app's
 *  own healthcheck probes are this value. */
export const APP_PORT = '5273'