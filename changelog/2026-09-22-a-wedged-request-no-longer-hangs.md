- **A wedged request no longer hangs a view until the page is reloaded.**
  The agents roster reported it — often on first load, the skeleton stayed
  for ever and only a refresh helped — but the hole was app-wide: browsers
  give `fetch` no timeout, so a request whose connection silently died (a
  dropped keep-alive after a server roll is the observed shape) stayed in
  flight for ever; the query above it never errored, so it never retried,
  and the surface waited on a skeleton nothing would ever claim. Two
  deadlines close it, both READS-ONLY by the same rule. Every read through
  the app's one HTTP door (`fetch-json`) now carries a 30s abort — a wedged
  read becomes an error TanStack Query retries, so the roster heals itself
  instead of waiting for a person to notice — and the SPA host's proxy to
  the Rust api carries a 30s deadline to FIRST RESPONSE on GET/HEAD: the
  stale-keepalive race a container roll leaves behind now answers a 502 the
  retry can act on. Writes are deliberately exempt everywhere — fleet verbs
  and reconcile block on docker for minutes, chat completions and assistant
  tool-call commands stream via postStream, uploads POST their bodies — and
  the proxy's deadline disarms the moment headers arrive, so the SSE relay
  and every other streaming body run unbounded. Verified end-to-end on the
  dev stack: with the api frozen mid-request (SIGSTOP), a proxied read
  answers 502 at the deadline and 200 in 15ms the moment the api resumes;
  the fake-timer tests pin all three halves (never-answering read → 502; a
  POST that never answers stays pending past ten minutes of fake time; a
  streaming body outlives the deadline).
