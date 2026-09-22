- **Marketplace installs for package-shipped MCP servers (npm, pypi, and
  docker/oci images) — the GitHub-and-friends long tail.** The official
  registry's package-only entries — over a third of sampled search results —
  were invisible to the marketplace ("packages that need a local process
  can't be one-click added"); now they install like anything else. Each
  install becomes one hardened `docker run` child of the api: stdio packages
  (npm via `npx` in stock node, pypi via `uv tool run` in the bundled-python
  uv image, oci images directly) pipe JSON-RPC through a pump this process
  owns, and oci packages declaring an http transport run detached and relay
  like any remote. The container carries the Hermes chassis posture minus
  its cap_adds (no-new-privileges, cap-drop ALL, pids/memory/cpus ceilings,
  pinned DNS, default bridge network — no fleet network, no postgres, no
  docker socket), registry-declared `docker run` flags pass an allowlist
  (volumes/env/mounts/hosts/publish only) enforced at install AND spawn,
  images pull at install and pin by digest, and credentials use the same
  Input schema as hosted headers — sealed at rest, materialized into the
  child's environment only at spawn, never on any GET. Installs show a
  third-party-code warning with an explicit confirm (strongest wording for
  community tier); v1 runs one org-shared container per package (per-user
  auth is refused). Lazy spawn with respawn debounce, tools refresh and
  gateway dispatch through the pump, stop on disable/delete, and a boot +
  5-minute reconcile that sweeps strays. Verified live: installing
  `mcp-server-time` (pypi/uvx) pulls and digest-pins the runner image,
  discovers `get_current_time`/`convert_time`, answers a gateway
  `tools/call` with real data through the hardened container, re-answers
  `initialize` from the cached handshake, stops on disable, and self-heals
  after an api restart with no stray containers; browser-verified the pkg
  badge, the community warning, env fields, and the acknowledgement gate.
