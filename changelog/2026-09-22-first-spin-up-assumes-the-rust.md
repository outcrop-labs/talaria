- **First spin-up assumes the Rust api — the hop is the default, not an
  opt-in.** `TALARIA_RUST_API_URL` unset now hops to the loopback default
  `http://127.0.0.1:5274` (the same port `talaria dev` and the production
  server-entry bind their api to), so a fresh checkout proxies the moment an
  api is listening — the coexistence posture of unset-forwards-nothing died
  with the TS routes it was protecting: post-cutover it served nothing but
  404s. The literal `off` stands the hop down for the one posture that wants
  no api behind the process (tests); every other posture — a set URL or the
  default — keeps the no-fallback rule, so an api that is down answers 502.
  `talaria setup` writes the URL into the generated `ui/.env`, `talaria dev`
  lifts it into the vite child unless the shell or `ui/.env` names one (an
  explicit value is never overwritten), healthz probes the effective URL
  (the default included; `off` skips), and server-entry honors `off` without
  probing or spawning. The docs cross with the code: RUST-MIGRATION's
  coexistence narrative is past tense end to end, and "Where it stands"
  records the cutover as landed.
