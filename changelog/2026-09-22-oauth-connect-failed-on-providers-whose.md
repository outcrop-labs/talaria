- **OAuth connect failed on providers whose dynamic registration refuses
  hosted callback URLs (Vercel).** Vercel's DCR endpoint allowlists
  redirect URIs to localhost and a few known clients, so any deployed
  Talaria's callback gets `400 invalid_redirect_uri` — which `ensure_client`
  collapsed to the unhelpful "client registration failed (400)" while the
  server card's manual-app escape hatch stayed hidden behind its
  `dcr: true` flag. A refused registration now persists a `dcrRejected`
  marker on the OAuth config, the connect error carries the upstream's own
  reason plus the exact callback URL to register, and the card shows the
  manual-app setup (its dashboard app accepts custom callbacks; saving
  credentials clears the refusal and restores Connect). Discovery also
  falls back to the protected-resource document's `resource_documentation`,
  so Vercel's setup banner links its real MCP docs. Verified live against
  mcp.vercel.com: connect under a hosted origin answers the actionable
  sentence and sets the marker; saving a manual client clears it and
  re-arms Connect; `oauth_meta` matrix + sentence pinned in tests.
