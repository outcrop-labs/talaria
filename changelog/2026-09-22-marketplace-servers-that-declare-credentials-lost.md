- **Marketplace servers that declare credentials lost their API keys on the
  way in.** The official registry declares remote headers as a `value`
  template ("Bearer {smithery_api_key}") with an optional `variables` map —
  and `classify()` parsed neither, so the install and per-user connect forms
  showed a bare header box, stored whatever was typed verbatim, and a pasted
  key left out the `Bearer ` prefix (upstream 401s); fixed publisher-set
  headers were dropped entirely, and the install POST's parser also stripped
  `isRequired`/`default`/`choices` from the stored declarations that drive
  the Settings → Connections form. The full `InputWithVariables` shape now
  flows registry → library wire → stored row → forms: a templated header
  renders one field per variable (secret-ness inherited, metadata from
  `variables`), the typed values are composed back into the final header, a
  literal `value` auto-applies with no prompt, and nothing is ever stored
  half-composed (`Bearer {key}` stays braces-intact until filled). Verified
  live: installing Smithery Notion from the marketplace prompts for
  `smithery_api_key → Authorization` and lands
  `Authorization: Bearer sk-…` on the server row; the per-user form renders
  the same field from the stored declaration; `bun run api:check` + `verify`.
