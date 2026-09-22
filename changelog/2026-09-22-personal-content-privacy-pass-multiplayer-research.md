- **Personal-content privacy pass + multiplayer research.** Personal-agent
  output is now private to its owner everywhere: PA-created documents, KB
  docs, research reports, and generated media carry the owner's
  `owner_user_id` and default to `private` (org agents keep publishing
  org-wide). Research is scoped like plans: you see your own runs, runs
  shared with you, and org-wide (agent-initiated) runs — and it's now
  **multiplayer**: a `research_members` table, `/api/research/:id/members`
  (share by email, owner-only, with notification + automatic editor grant
  on the report artifact; collaborators can leave), and an avatar-stack
  share UI in the run header mirroring plan sharing. Briefings only
  surface research you own or were invited to.

### Fixed
