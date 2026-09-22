- **Docs overhaul, part 1 of 5 — restructure + truth** (first PR of the docs pass: dev docs and
  end-user docs, succinct and table-first). Ten dated/superseded docs moved to
  `docs/history/` (PLAN, PRICING, SECRETS-PLAN, PRODUCT-GAPS/PRODUCT-PLAN, three audits,
  m0-contract, TODO) with a README stating the rule: nothing there is current, the live doc wins —
  every inbound link repointed. New `scripts/check-docs.mjs` tripwire: every markdown link in the
  tree must resolve to a real file (would have caught the `m0-contract → adapter/` link that
  pointed at a deleted directory for months); wired into `bun run check`. `apps/README.md`
  rewritten to the real Svelte 5 SDK anatomy (was React-era: tsx/useQuery/react imports);
  `ui/README.md` setup section now shows the one true path (`bun talaria setup` from the repo
  root), drops its roadmap-duplicate sections, and documents per-agent `tak_` credentials instead
  of the retired org-wide `TALARIA_AGENT_KEY`.
