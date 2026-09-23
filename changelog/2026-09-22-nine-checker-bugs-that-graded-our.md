- **Nine checker bugs that graded our gaps as model failures — and one
  guard widened only after its own trap test vetoed two tokens.** The
  sweep's headline fixes: the json-strict gate's verify step tested closure
  the wrong way round (a strict failure looked verified), and unstamped
  pre-revision records now self-heal on read rather than failing every model
  forever; the workbench's stale-grade regex matched "NOT FINISHED" grades
  as processed (`NF[KD]?D` on a grade that also reads "NOT DONE"), with a
  scripted repro pinning it; the judge gate rework carried both. The
  research gap-round checker now catches the question re-asked as a query
  by token containment (all but ≤4 of the question's own words) — the
  commonest repeat matched neither literal and re-trod the findings for
  free, while a query naming new ground cannot carry those words and stays
  legal. Six too-strict alternations widened, each anchored on words that
  survive paraphrase and each with a trap test: a deferral recorded as
  "postponed"/"no longer" no longer fires plan_doc's both-positions check
  (the widening dropped `revisit` — CURRENT_DOC's own "revisited twice and
  settled" HOLDS the decision — and `staying`, which qualifies the reversal
  line, not the Postgres line), "notify customers by email" places the comms
  plan, "left out"/"discarded" name an omission, a refusal worded "turned
  down"/"vetoed"/"ruled out"/"passed on" is recorded, "I verified" grounds a
  live-state count, and a brief about "the shared secret"/"the credential"/
  "the HMAC check" engages with its item. Also honest now: `list_files` and
  `write_file` report real bytes (the sandbox said "bytes" and returned
  UTF-16 code units — identical on today's ASCII fixtures, a trap for the
  first fixture with an em-dash), the stale FORTY-SIX tool counts read
  FIFTY-FIVE (the number the sync test pins), and the timeout verdict names
  which clock fired — a model idle for minutes is not a model that was
  slow, and the inactivity clock now says so instead of letting a hung
  upstream read as the model's timeout. Verified: each fix carries a test
  that fails on the old pattern (the research repeat, the plan_doc
  paraphrase, the distiller refusal quartet, the inbox grounding and brief,
  the workbench repro, the judge inversion); full lib suite 1952 green.
