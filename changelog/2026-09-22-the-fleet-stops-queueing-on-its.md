- **The fleet stops queueing on its own database.** Nothing in the API rate
  limits agents — but four mechanical throttles made parallel load *feel*
  throttled, and all four are gone. The pg pool grows from the inherited 10 to
  40 (`TALARIA_PG_POOL_MAX`, clamp 1–200) with a 15s acquire timeout
  (`TALARIA_PG_ACQUIRE_TIMEOUT_MS`) so bursts queue instead of 500ing at 5s;
  the UI's own pool moves from a hardcoded 10 to `TALARIA_UI_PG_POOL_MAX`
  (default 20), both wired through compose's environment block (api 40 + ui
  20 + migration 1 = 61 of the sidecar's 100). A warm agent turn — the whole
  reason for the Rust port — stops paying ~7 serial pool checkouts before the
  upstream call: the endpoints table serves from a 15s window (every
  in-process writer drops it), the endpoint key decrypts once per 60s
  (rotation invalidates), `tlk_` identities resolve from a 15s cache that
  negative-caches unknown keys (revoke/policy edits reset it in-process), and
  the gateway's four hot `app_settings` reads serve from a 15s layer that
  `set_setting` invalidates. The OpenRouter US-pool fetch is single-flight
  with a 60s failure backoff — one TTL lapse under N concurrent turns makes
  one external fetch, not N × 10s hangs — and the shared HTTP client gains a
  10s connect timeout. healthz reports `pgPool` (size/idle/max) and warns when
  the pg ping lands slow with the pool drained, so future saturation is
  visible rather than guessed at. And the Inbox's per-user turn lock comes
  off the panel's own reads and state writes: GET /conversations/{id} serves
  mid-turn (MVCC snapshots can't read half a write; the page's `working` flag
  renders the in-flight reply) and a snooze mid-stream is a state change, not
  a conflict — the lock stays on command and action POSTs, where seq
  allocation and decision execution actually race.

### Fixed
