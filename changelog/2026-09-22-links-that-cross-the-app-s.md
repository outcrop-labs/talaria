- **Links that cross the app's boundary land where they claim.** Two href
  writers still emitted the pre-path-routing query shape after tabs and
  selections became path segments: a research run's completion notification
  pointed at `/research?r=<runId>` (which opens the bare research view with
  nothing selected — the quiet wrong answer) and a retrieved knowledge
  document pointed at `/knowledge/<docId>`, a one-segment form the Knowledge
  route parses as *space* with nothing selected. The writers now emit
  `/research/<runId>` and the by-id permalink `/knowledge?doc=<docId>`. The
  repair ships as the migration law's first exercise: two appended statements
  rewrite the stored rows (`notifications.href`,
  `briefings.source_href`) from `/research?r=` to `/research/` — idempotent
  and WHERE-guarded, applied at boot like every other one-shot. What a
  migration cannot reach — the href baked into emails already sent, and
  Qdrant payload hrefs until the next reindex regenerates them — is caught
  by a `?r=` redirect in the app shell, the same net `?tab=` already casts
  for the same reason.
