- **A decision-model port, off by default, that runs on whichever model you
  want.** Talaria asks a text model to *decide* things in a dozen places, and
  every one of them pays the same tax: a prompt spelling out the answer shape
  in English, a JSON parse, a repair turn, a normalizer folding `"yes"`/`"true"`
  back into a boolean — and at the end of it a bare answer with no number on
  it. A decision model answers the shape by construction and hands back a
  calibrated probability. `talaria-decide` is the port for one: three
  primitives (`Noul` yes/no, `Choice` pick-one, `Score` how-much), asked
  together over one state, each answer carrying its own confidence.

  It is a **registry, not an integration.** Four providers ship — TypeSafe's
  Jev, a classifier on your own hardware through the TEI sidecar this repo
  already runs, any model you already registered on Models, and `custom`: a
  URL plus a declaration of which of the three wires it speaks, so a System One
  competitor or an in-house service needs no code from us. There is no blessed
  default and no recommended model; the registry instead carries a **capability
  sheet** per provider — which question shapes it answers, whether it answers
  several in one round trip, and where its probability actually comes from — and
  `decide` refuses a question a provider cannot answer rather than letting it
  guess. Admin → Agents leads with the panel, including a Test button that puts
  one real question through whatever is configured and reports the probability,
  the latency, and whether the confidence is a real number at all.

  **Nothing consumes it yet, and `off` is the default.** The port answers
  `Option` — `None` means "run your own path" — so a call site keeps the branch
  it already had, and with the port off, misconfigured, unreachable or answering
  below a threshold, every surface runs exactly the code it ran before. The
  companion rule is `calibrated`: a provider that answers without a real
  distribution behind it (a chat endpoint that drops `logprobs`, most commonly)
  is still usable as an *answer*, and `gated` refuses to let any threshold read
  its confidence, because a fabricated number is worse than no number — it
  looks like evidence.

  Verified: 48 new unit tests in `talaria-decide` (the three wire shapes'
  builders and readers pinned against recorded replies, the confidence gate, the
  provider registry, config sealing and the patch fold) plus 2 in
  `admin_decide`, all passing via `cargo test -p`; `cargo clippy -p
  talaria-decide -p talaria-routes-admin --all-targets` clean; `bun run
  typecheck` (svelte-check, 5432 files) clean; `bun run check` clean with the
  regenerated API reference (300 routes).

  Exercised end to end against a running worktree stack, with a stand-in
  endpoint speaking each wire so the whole path — config row → dispatch → wire
  → parse → `Judgment` — ran for real rather than against fixtures. The
  `systemone` request that actually went over the wire carried the documented
  shape (`state` object, `questions` map, `criteria: {true, false}`) verbatim; a
  noul of 0.93 came back as certainty 0.86, and the `predict` wire's
  entailment/contradiction scores of 0.88/0.04 came back as 0.957, both matching
  the arithmetic by hand. The fallback paths hold: an unreadable 200, and a port
  pointed at a dead socket, each answer "did not answer" rather than 500ing. A
  wire or provider we do not speak is a 400 and never becomes a `configured`
  row; an unauthed GET is 401. The key seals (`v2:` envelope in `app_settings`),
  never appears in any response, unseals and reaches the endpoint as a bearer,
  and clearing it returns the row to exactly `{"provider": "off"}`. The panel's
  declared TypeScript types were checked key-for-key against the live response.
  Not verified: the panel's rendered appearance — this session had no browser.
