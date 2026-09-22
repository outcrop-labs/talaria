- **Input consistency sweep (#49).** Every form control now speaks the same
  keyboard language, via two tiny shared helpers (`submitOnEnter`,
  `inlineEditKeys` in `ui/control.ts`) instead of hand-rolled `e.key`
  handlers: inline title/name edits (KB space + doc, artifact, ticket) commit
  on Enter and revert on Escape — Escape is shielded from modal/document
  handlers so cancelling an edit no longer risks closing the surface;
  field-plus-button rows submit on Enter (provider API key, assistant handle
  rename, agent secrets, MCP server add, rerank key, audit retention,
  federate directory, version note); the home-dashboard email compose — the
  one dialog still hand-rolling its own backdrop and raw inputs — now sits on
  the shared Modal (Escape/backdrop close for free) with the shared field
  primitives and focuses "To" on open; and the login username, provider-create
  name, and save-image title fields focus on open. Audited ~80 text controls
  across 33 files; 16 gaps fixed, the rest already followed the conventions.

### Added
