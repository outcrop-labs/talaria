- **Self-hosters — env auth is removed entirely.** `AUTH_USERS`, `AUTH_PASSWORD_ENABLED` and
  `AUTH_ADMIN_EMAILS` are ignored after this upgrade (no import path). A fresh instance has
  zero users: the first visit offers `/claim`, and the account created there (email + password,
  or the first Google sign-in when Google login is enabled) becomes the admin — first claim
  wins, so reach the claim screen before exposing a fresh instance publicly. Existing users
  keep their stored roles; re-create password sign-in via Admin → People (or Google).
  `AUTH_ALLOWED_*` gates Google sign-ins only now — password accounts are admitted by the
  admin who creates them.
- External SDK consumers only: API renames — `/api/plan/:id/doc|draft` → `/api/plans/…`; singular
  `/api/inbox/focus/conversation` folded into `/api/inbox/focus/conversations/$id` (with `current`
  sentinel); `/api/profile` → `/api/me`. Validation-failure bodies are now zod-issue 400s across
  ~90 routes (was a mix of 400/422/custom); 401/403 split fixed on four task endpoints; 500s no
  longer echo `e.message`.

### Added
