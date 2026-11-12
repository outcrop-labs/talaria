# judge-review fixtures

Data for `scripts/judge-review.test.mjs` and the weekly canary — no tests live here
(`scripts/run-self-tests.mjs` does not recurse).

- `sse-*.txt` — OpenRouter chat-completion streams, **synthesised** on 2026-11-11 from
  OpenRouter's documented SSE format (`: OPENROUTER PROCESSING` keep-alives, `data:` chunks,
  a final usage chunk with `cost`, `data: [DONE]`). They are to be replaced by the first
  calibration recordings (`judge-calibrate` uploads them, reasoning stripped). A recording
  never holds reasoning, request headers or the key.
- `model-outputs/*.json` — raw `findings` arrays a model might return, each built to hit one
  filter in `filterFindings` (a fabricated quote, an ambiguous one, a short one, a generated
  file, an outside-the-hunk anchor, a deletion-only anchor, low confidence, an uncited rule,
  a `rule_id`, duplicates, ten findings, injected markup and bidi characters). They quote the
  scratch repository the test builds (`api/crates/demo/src/lib.rs`).
- `canary/{defect,clean}/{base,head}` — the canary's two changes. `defect` seeds a SQL string
  built from a request parameter and a delete that lost its owner check (`seeded.json` names
  the file); `clean` is a correct refactor.
- `calibration-results/<pr>-<sample>/review.json` — saved reviews for `score --results`.
- `ledger/` — a synthetic ledger and grades for `score --ledger`.
