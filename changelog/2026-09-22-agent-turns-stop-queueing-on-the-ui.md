- **Agent turns stop queueing on the UI and on a 500ms Redis timeout.** A
  16-core box with many agents in flight was idle because every LLM stream
  and every toolkit call went through one single-threaded UI process, and
  redis-rs 1.6 times out the one shared ConnectionManager at 500ms — lease
  renewals fail, runs stall, agents retry into the same queue. Container
  mode now binds the API on the fleet network (`TALARIA_API_BIND=0.0.0.0`,
  not published to the host) and points `TALARIA_GATEWAY_SELF_URL` at
  `:5274`; app MCP stays on `:5273` because those servers are TypeScript.
  The toolkit child dials `127.0.0.1:5274` instead of the UI. The Redis
  command timeout is 5s (`TALARIA_REDIS_RESPONSE_TIMEOUT_MS`). The api pool
  default moves 40 → 64, and the sidecar starts postgres with
  `max_connections=200` (a restart applies it to an existing volume);
  64+20+1 still fits the old 100, including the superuser reserve, so an
  API restart that lands first cannot exhaust postgres. A rendered fleet
  keeps the old gateway URL until the next render and roll.

### Fixed
