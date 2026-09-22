- **Talaria is an app platform.** Apps are self-contained codebases in
  `apps/<slug>/` that compile INTO the deployment and load as native
  surfaces — work views, manage views, settings tabs — on the same
  router, design system, and session as core. Each app can ship its own
  API (`/api/apps/<slug>/*` with an authenticated user handed to every
  handler), a migrations-free per-app document store, and MCP tools for
  agents that register in the MCP registry (badged "app") under the SAME
  granular governance — per-agent assignment, tool subsets, per-person
  access, gateway-enforced, dispatched in-process. Apps are
  explicit-grant: enabling one gives members nothing until an admin
  allows its views per person. Manage → Apps has Installed + Discover:
  a marketplace catalog (community + official, configurable index) and
  install-from-any-git-URL — a shallow clone into `apps/` IS the
  install; dev picks it up live, prod flags "awaiting build". Built on
  `@talaria/sdk` (docs/SDK.md); `apps/contacts` is the reference.
