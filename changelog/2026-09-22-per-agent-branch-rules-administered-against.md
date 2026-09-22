- **Per-agent branch rules, administered against the repo's real branches,
  enforced by the platform before a byte leaves the container.** Every repo
  grant now carries its own branch law: the base branch a human merges
  (picked from the repo's live branch list, defaulting to its default
  branch), whether the agent may push the base at all (`branches_only` is
  the zero-config default posture — work lives on a branch; `free` is the
  explicit opt-in), and an optional required prefix for the agent's
  branches. The gates pass down the whole chain: the admin panel edits rules
  beside the grant chips with real branch pickers; the work-session brief's
  hygiene step is personalized from them (each granted repo's base and
  prefix, with the standing default when unconfigured); and a rendered
  `pre-push` hook — installed system-wide via `core.hooksPath`, chaining to
  a repo's own hook when one exists — asks `/api/secrets/git-push-check`
  over the agent's key with the refs git is about to push, so the platform
  itself declines a base-branch or off-prefix push even on repos GitHub's
  free plan cannot protect. Grant toggles no longer wipe rules (set
  semantics, not delete-and-reinsert). Verified: the decision table is
  pinned by unit tests, the generated hook is proven valid shell by `sh -n`
  in the test suite, and live from an agent container a base push is
  declined with the rule's own sentence while a conforming branch push
  passes.
