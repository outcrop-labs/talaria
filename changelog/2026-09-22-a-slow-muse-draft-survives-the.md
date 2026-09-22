- **A slow Muse draft survives the proxies between the browser and the api.**
  The structured kinds (the New Agent modal's design call, cron drafts,
  ticket patches) put nothing on the wire until the whole validated JSON
  exists — and a genuinely slow generation (outcrop 2026-09-14: a 2m20s
  agent design on a cold provider day) ran into Cloudflare's 100s idle
  ceiling and came back to the user as a timeout, while the api finished the
  turn for nobody. A draft that runs past 45s now opens its `application/json`
  body and drips `\n` heartbeats — leading whitespace is legal JSON, the
  client's parse cannot tell — ending with the same object the buffered path
  sends. Fast runs (the overwhelming case) answer byte-identically to
  before, statuses included; the answer's error split (400 configuration /
  502 model / 500 thrown) is pinned by test. The design modal also counts
  aloud now: past ten seconds the progress label shows elapsed seconds, so
  "working, slowly" stops reading as "wedged".
