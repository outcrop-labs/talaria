- **One owner per duplicated helper** (the audit's recurring failure mode): Google OAuth built once
  (`server/google/oauth.ts`), JSON-RPC envelope once (`mcp-jsonrpc.ts`), `errText`/`errLine`,
  `tz.ts`, `docker-exec.ts`, `asIso`, shared zod schemas (`lib/api-schema.ts`); `localMoment`,
  board-visibility SQL, MCP protocol pin consolidated. Client mutations all through
  `fetch-json.ts` (`postJson`/`putJson`/`patchJson`/`delJson`) — and the `credentials:
  'same-origin'` stanza is now census-enforced to live only there.
