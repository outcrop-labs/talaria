- **MCP OAuth 2.1, end to end.** Servers that answer 401 get the full
  spec treatment: protected-resource metadata → RFC 8414 authorization-
  server metadata (path-aware, so github.com/login/oauth resolves) →
  dynamic client registration → PKCE, with resource indicators. No-DCR
  providers (GitHub) get a manual OAuth-app flow: Talaria shows the
  callback URL to copy and links the provider's own app portal from its
  service_documentation. Tokens seal per subject (org or user), refresh
  silently, and force a visible reconnect on revocation. Registry changes
  that alter what a running agent carries roll the fleet blue/green —
  Hermes wires MCP at process start, so a render alone was never enough.
