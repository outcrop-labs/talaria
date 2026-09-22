- **W4d — one teams-directory read.** `useTeamsDirectory(enabled)` in
  `lib/teams.ts` replaces four hand-rolled reads and the dead `useTeamDirectory`
  (zero callers, same endpoint, different key). The key moves from
  `['teams-directory']` to `['teams','directory']`, so the membership writes that
  already invalidate `['teams']` now refresh the directory too; the wire shape is
  the real one (`TeamDirectoryEntry { id, name, memberCount, agentCount }` — there
  is no `kind` field, and the four consumers each declared a different slice of
  the same row). The optional `enabled` gate is behaviour, not decoration: the
  channel settings modal read this only while open.
