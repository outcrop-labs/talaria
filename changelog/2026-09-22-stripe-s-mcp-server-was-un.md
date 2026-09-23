- **Stripe's MCP server was un-connectable: OAuth discovery never found its
  authorization-server metadata.** Stripe's issuer URL carries a path
  (`https://access.stripe.com/mcp`) and serves metadata at the RFC 8414
  location — the well-known segment before the path — which was the one
  shape `discover_oauth` didn't try, so Stripe installed as a plain
  header-auth server with no Connect flow at all. The candidate set (now
  extracted and test-pinned) tries every well-known shape; probed the other
  marketplace majors while in there — Notion, Linear, Airtable, and PayPal
  register hosted callbacks out of the box, Figma and Asana refuse DCR and
  land in the manual-app flow above, and GitHub keeps its documented
  cross-domain pin. Verified live: registering `mcp.stripe.com` now
  discovers OAuth (`dcr: true`) and a connect start 302s into Stripe's
  authorize endpoint.
