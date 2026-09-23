- **Reasoning effort reaches the primary chat surfaces.** Comms agent DMs and
  the assistant panel now offer an effort chip on the composer rail, right
  aligned just left of the send tile — but only when the model's own catalog
  metadata vouches for levels. OpenRouter-style catalogs publish
  `reasoning.supported_efforts` per model; that list is extracted into the
  stored per-model catalog metadata at the same moment an admin adds models on
  /models (the live-catalog refresh the model adder already triggers), so the
  picker lists exactly the levels the provider says the model accepts and
  nothing else. A model that publishes no levels gets no chip, and its requests
  carry no `reasoning_effort` at all. Server-side, `/api/chat` and the inbox
  focus command validate the pick against the same metadata (a persona id is
  resolved to the model actually serving it, tiers included), send it as
  `reasoning_effort` on the outbound turn, and honor the effort of a QUEUED
  message on the chained turn that covers it (stamped on the message row,
  re-validated before the chain runs). Tool-offering gateway turns keep the
  `'none'` workaround — function tools and an effort cannot share a request.
  Verified: `npx tsc --noEmit`, `npm test` (2476 passing, including new
  coverage for the catalog extraction, the pool intersection, the persona
  resolution, and the transport bodies), `npm run typecheck`, plus a live
  catalog read against OpenRouter confirming every `supported_efforts` list
  arrives on a model that also advertises the `reasoning_effort` parameter.
