- **Confab guard: annotate and strict modes now act.** They were configurable
  but every path discarded the result — observe was effectively the only mode.
  Annotate pins findings to the flagged reply (`messages.guard` /
  `channel_messages.guard`) and renders a warning caveat under it in chat and
  channels (channels update live via republish); the public LLM route appends
  the caveat to non-streaming responses and injects one final SSE delta before
  `[DONE]` on streams. Strict additionally redacts detected secrets (keys,
  tokens, whole private-key blocks) from whatever Talaria persists or hasn't
  yet relayed, so saved copies and future transcripts stay clean. Agent-loop
  keys (`gateway_unmetered_keys`) never receive caveats — a finding must never
  re-enter a model's context; internal utility completions (judge, muse,
  research) likewise stay observe-only so parsed outputs can't be corrupted.
  Admin copy now describes what each mode actually does.
