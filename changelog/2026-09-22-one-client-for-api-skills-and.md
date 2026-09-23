- **One client for `/api/skills` — and the stale list it caused.** Two hooks
  backed one endpoint: `lib/skills.ts`'s `useSkills()` under key `['skills']`,
  and `lib/workflows.ts`'s `useSkillLibrary()` under `['skill-library']`. Each
  surface invalidated only its own key, so renaming or deleting a skill from the
  Studio left the Skills library showing the old name until a manual refresh.
  `workflows.ts`'s second client is gone; `Studio.svelte`, `StudioGuide.svelte`,
  `WorkflowDetail.svelte` and `SkillRow.svelte` now read `useSkills()` and
  invalidate `SKILLS_KEY`, so any surface's write updates all of them.
  `SkillOwner` carries the `label`/`model` the Studio rows read.
  Verified: `bun run verify` (svelte-check 0 errors, 1,183 tests). The rename
  round-trip itself was **not** exercised in a browser — no docker on this host,
  so the dev stack cannot be started; the fix is the shared key, which is what
  `SkillEditor`/`SkillsLibrary` already invalidate.
