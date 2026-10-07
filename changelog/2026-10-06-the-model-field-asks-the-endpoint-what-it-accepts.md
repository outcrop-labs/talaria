- **The model field now asks the endpoint which ids it accepts, and a refusal
  is quoted rather than paraphrased.** The first real configuration of the
  decision-model panel failed, and it failed in the most ordinary way
  available: `model` was typed as `jev` rather than `jev-latest`. Three things
  had to line up for that to be hard to fix, and all three were ours — a
  free-text field whose shortest plausible value is wrong, a `<datalist>` of
  ids we had written down by hand (which *suggests* and cannot *contradict*),
  and a Test button that answered with our generic sentence while a
  `400 Unknown model: jev` sat unread in the response body.

  **`POST /api/admin/decide {"action":"models"}`** asks the configured endpoint
  for the ids it will take. The panel asks on load for a provider that
  publishes a catalog, offers the result in the datalist, and — the part that
  would have saved the afternoon — says so plainly when the configured id is
  not among them. The field stays **free text**: the whole position of this
  registry is that an operator runs whichever model they want, including one
  shipped after we were, so detection informs and never refuses.

  **Per wire, and the differences are not cosmetic.** `systemone` reads
  `GET /v1/models` → `{"models":[{name,description,release_date}]}`, which are
  TypeSafe's own field names from its OpenAPI document and **not** the OpenAI
  convention — a reader that looked for `data[].id` would have reported "no
  models" against a working endpoint. `chat` reads `GET {base}/models` in the
  three containers `talaria_gateway` already meets in the field (`data`,
  `models`, a bare array). The classifier wire is asked **nothing**: it serves
  the single model it was started with, `needsModel` is already false for it,
  and inventing an `/info` read we have never seen a reply from is exactly the
  guess this change exists to delete. And the registered-model provider is
  answered **without a request** — the operator already refreshed that
  endpoint's catalog on /models, so that row *is* the detection; asking twice
  would be two sources of truth for one answer.

  **`jev` is now `live_catalog: true`**, because it provably has the endpoint.
  The three documented ids stay as the fallback for when the call cannot be
  made.

  **The refusal is the provider's own.** `decide`'s contract is unchanged and
  deliberately still `Option` — a call site does the same thing whatever the
  reason, and one that branched on it would be making a policy decision the
  port does not own. `try_decide` keeps the reason for the one caller whose
  entire job is to render it, and `NoAnswer` distinguishes not-configured,
  unusable, unreachable, refused-with-its-own-words, and a 2xx in a shape the
  wire does not recognize. `upstream_message` reads four envelopes, **each one
  observed rather than assumed**: TypeSafe's `{"detail":{...,"message"}}` (its
  auth and model refusals), FastAPI's `{"detail":[{"msg"}]}`, a bare
  `{"detail":"Not Found"}`, and OpenAI's `{"error":{"message"}}`. Anything else
  falls back to the body, clipped — bytes we cannot parse are still more
  informative than a sentence of our invention, which is the failure the whole
  function undoes.

  The gateway provider's misconfigurations got sentences too, since they were
  the other silent `None`: a model id no registered endpoint serves, and an
  endpoint hint that matches nothing, each now say which.

  Verified: 69 tests in `talaria-decide` (4 new — the four refusal envelopes
  including the two auth bodies captured verbatim from a real
  `api.typesafe.ai` response, the unparseable fallback, TypeSafe's catalogue
  read by its own field names from its published `ModelMetadataList` example,
  all three chat containers, and a catalogue of nothing reading as empty rather
  than as blank rows); 11 in `talaria-routes-admin` (1 new — the refused case
  carries `Unknown model: jev` through to the sentence, and no two kinds share
  a tag). clippy clean across both; `bun run typecheck` 0 errors.
