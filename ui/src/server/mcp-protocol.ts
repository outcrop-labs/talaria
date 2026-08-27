/** THE ONE MCP PROTOCOL REVISION TALARIA SPEAKS — as a client (the probe, the
 *  registry's tool calls and refreshes, the OAuth handshake probe) and as a
 *  server (the app and workbench dispatchers' fallback answer).
 *
 *  Those five call sites carried the literal separately and had already split
 *  — the probe said `2025-06-18`, everything else `2025-03-26` — and nothing
 *  was broken only because MCP servers tolerate both.
 *
 *  `2025-03-26` deliberately: it is the revision OUR dispatchers answer with
 *  when a client doesn't name one, so pinning the clients there too makes one
 *  revision serve both directions of the same conversation. Bumping this is a
 *  protocol decision, not an edit — every site moves together or none does.
 *
 *  A leaf (not mcp-registry.ts, which owns the other MCP constants) because
 *  mcp-oauth.ts cannot import the registry: the registry imports IT for the
 *  token reads. */
export const MCP_PROTOCOL_VERSION = '2025-03-26'
