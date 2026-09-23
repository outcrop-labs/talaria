- **The bundled Hermes github skill is pruned from fleet containers, and a
  Talaria-authored `github` skill stands in its place.** The image ships a
  gh-CLI-first github pack whose preflight (`gh auth status`) and auth
  workflows pitch exactly what Talaria forbids — there is no `gh` in the
  containers, and the credential is injected at git time, never visible.
  Observed live on outcrop (2026-09-15): an agent opened the skill, noted it
  "assumes gh CLI", and recovered only because its memory notes said
  otherwise. The pack joins `CONFLICTING_SKILL_PACKS` (pruned on every
  container roll, the same mechanism as the five note-tool packs), while
  `scripts/skills/github` seeds the shared root with a signpost skill at the
  exact name agents reach for — plain git over https, the workbench opens
  the PR, no auth to set up — routing to the talaria-toolkit and
  workbench-driving sections that carry the methodology. Verified in an
  isolated worktree render: the skill seeds into the fleet shared root and
  lists as a platform (admin-locked) skill beside the toolkit; the prune
  path matches the live dogfood container's pack layout.
