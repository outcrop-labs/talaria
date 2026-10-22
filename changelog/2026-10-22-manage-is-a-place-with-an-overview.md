- **Changed:** Manage is a place now, with an overview, instead of a pane that slid out over whatever you were doing. The gear moved out of the row of work views to the right of the top bar, beside the assistant and next to the account menu that holds Settings and Admin. It goes to the new `/manage` overview, and so does the user menu's Manage entry. Every manage page (Agents, Teams, Models, MCP, Templates, Agent Studio, Observability, Apps, and each app's manage surface) keeps the Manage sidebar docked on its left. The sidebar has no pin and no overlay, and it is absent from work views; below `md` its rows become a strip across the top. The overview shows four things:
  - **Needs attention.** The instance's own alerts, worst first, each linking to where it is fixed. Shown to admins and people granted Observability.
  - **Tiles.** One per area the viewer has been granted, each opening its page:
    - agents up, with the down, degraded and warming beside it;
    - spend today and this month;
    - model providers, with any missing a key and any model role left unrouted;
    - MCP servers needing attention;
    - enabled apps and failed builds;
    - teams, templates, and skills with their open suggestions.
  - **Controls, for admins.** Roll every agent (with the same confirm and "also stopped" option as the Agents page), start stopped agents, and check every MCP server now instead of at the hourly pass.
  - **Recent changes.** The newest rows of the audit trail.

  A tile renders, and sends its request, only when the viewer holds that view and the endpoint's own permission. A member with no manage grants gets no gear and an empty state.

  Verified: `cd ui && bun run typecheck` (0 errors, 0 warnings), and `bunx vitest run` for the new `manage-area` suite and `nav-sections`. In Chromium against the worktree stack, as an admin at 1440×900:
  - the gear sat in the right cluster, lit on `/manage`, `/agents`, `/models` and `/observability` and unlit on `/work`, where no sidebar showed;
  - clicking it from `/work` opened the overview with the sidebar docked at 208px;
  - a sidebar row opened Observability with the sidebar still there;
  - all eight tiles rendered with live numbers;
  - Roll all showed its confirm and answered "Nothing to roll.", and Check now answered "No external servers to check.";
  - Start stopped rendered the started agents from an intercepted answer, because a real call would start containers on the host.

  At 390×844 the rows became a strip and nothing scrolled sideways. As a member granted Agents and Templates, the sidebar listed Overview, Agents and Templates, two tiles showed ("none you manage" for an empty roster), the controls and alerts panels did not render, and `/models` redirected home.
