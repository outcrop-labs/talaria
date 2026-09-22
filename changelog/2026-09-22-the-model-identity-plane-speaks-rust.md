- **The model-identity plane speaks Rust, closing batch 3**: `/api/models`
  (the picker catalog — gateway rows carrying label and blurb from
  OpenRouter's public catalog, plus the persona-resolved `effective` model),
  `/api/models/efforts` (the composer's effort ladder for a persona or
  catalog id, with the agent-configured default held against the levels the
  model actually publishes), and `/api/admin/model-roles` (the Model Roles
  panel — assignments, fitness issues, per-role effort preference, GET and
  PUT). The three stand on one harness persona engine, so they crossed as
  one slice and the engine came with them: the persona index's two-pass
  claim (base ids first; alias tiers second with NO main-configured gate; no
  main, no keys; every pool carries the fallbacks the runner can land on),
  the effort ladder (provider catalog plus the admin declaration, which
  replaces and never merges, with the once-only backfill for catalogs stored
  before effort extraction existed), the model-info matcher (6h
  stale-on-failure cache, `model_blurbs` override whose read error
  propagates exactly as TS's `Promise.all` does), and the resolution chains
  behind `effective` (pin→role→utility→env→first-routable; the member
  allowlist gates every link except pin and env). Verified by a 37-case
  byte-diff against TS on the same sessions — the full zod 400 table behind
  the PUT, restore-safe write cycles run through both runtimes with the
  audit rows mirroring, the 405s with their `allow` headers, a duplicate
  `?model=` (URLSearchParams.get is first-wins), and the persona-configured
  default read through a live config mutation after both runtimes' persona
  caches expired. Three cases differ in array order alone — the recorded
  localeCompare divergence, seen for the first time now that an endpoint
  name carries a capital letter (TS collates `Z.ai/glm-5.3` last, byte
  order puts it first). Two divergences recorded: the org-voice blurb
  sweep's only TS trigger was the `/api/models` handler the proxy now
  shadows, so it is dark in proxied environments until it ports as a real
  scheduler job (batch 4) — Rust reads the rows, never writes them — and
  the blurb clamp cuts at a char boundary where TS's UTF-16 `slice` can
  split a surrogate pair.
