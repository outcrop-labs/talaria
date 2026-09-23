- **Navigation cleanup.** The sidebar is work surfaces only: the bottom
  logo is gone, the System section is gone — Settings and Admin live under
  the user menu (Admin still hard-gated by role independent of what the
  rail renders). **Compute, Cost, Audit, and Alerts merged into one
  Observability view** (`/observability`): the root is an Overview —
  alerts strip worst-news-first, generating-now + fleet temperature,
  gateway pulse, spend today, recent audit trail, each clicking through
  to its detail tab (`?tab=compute|cost|audit|alerts`). The four old
  routes are DELETED, not redirected — every reference (nav, home fleet
  tiles, server-side notification hrefs, the activity indexer) was
  rewritten to the new structure. **Models is tabbed**: Models (provider
  registry) · Roles · Access, deep-linked, with the verbose panel
  descriptions and field explanations moved into ⓘ tooltips.

### Added
