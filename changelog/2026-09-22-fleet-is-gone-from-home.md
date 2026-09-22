- **Fleet is gone from Home.** The Fleet tab (admin-only) and its pulse view
  are deleted — Agents shows the same running state, and Observability owns
  the deep view, so a third copy was overhead nobody normal asked for. The
  home summary no longer computes fleet health (one fewer container status
  pass on every load), and the assistant's surface map no longer knows a
  Fleet destination. Old links keep working: `/fleet` redirects to /agents,
  and `/home/fleet` falls back to the Inbox like any unknown tab. Verified
  live: `/api/home` returns no `fleet` key, the Home tab strip renders
  Boards/Comms/Plans/Research/Docs with no Fleet entry for an admin, and the
  surface tests (including the `/home/fleet` fallback pin) pass.

### Fixed
