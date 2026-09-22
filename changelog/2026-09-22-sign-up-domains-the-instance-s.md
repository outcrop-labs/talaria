- **Sign-up domains + the instance's own address.** Two kinds of domain,
  deliberately separate. EMAIL sign-up domains (Admin → Org): prove
  ownership via a DNS TXT record at `_talaria-verify.<domain>` and anyone
  with a matching address may self-join — verification is mandatory, so
  nobody claims gmail.com. The HOSTING domain verifies by a self-fetch
  round trip: the server requests its own identity beacon through the
  candidate domain and checks the instance id that answers — proof DNS,
  routing, and TLS land on THIS deployment. Once verified it is the
  canonical base URL for MCP OAuth callbacks and links.
