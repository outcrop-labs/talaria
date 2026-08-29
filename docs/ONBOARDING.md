# Onboarding — domains, invites, transactional email

How people get into a Talaria instance, and how the instance knows its own address. Three doors in,
one hosting identity, one email pipe. All admin surfaces live in Admin → Org / Admin → People.

## The three admission doors

A Google sign-in is admitted when ANY of these passes:

1. **Env allow-list** — `AUTH_ALLOWED_EMAILS` / `AUTH_ALLOWED_DOMAINS` (a Google-only gate).
2. **Verified email sign-up domain** — anyone with an email on a domain your org has proven it
   owns may self-join.
3. **Invite** — a live invite exists for that exact address.

Password accounts are a fourth door, but not a self-serve one: an admin creates them in
Admin → People. And before any of this, a fresh instance has one more door — `/claim`,
where the first person through becomes the admin (see `docs/user/getting-started.md`).

## Email sign-up domains (Admin → Org)

Add a domain (e.g. `getboxie.com`) → Talaria issues a TXT token → publish it in DNS at
`_talaria-verify.<domain>` → Verify. Verification is **mandatory** before self-joins open (it's
what stops someone claiming `gmail.com`). Removing a domain stops its self-joins immediately.

The email domain is deliberately **separate from the hosting domain**: your people sign in with
`@getboxie.com` addresses while the instance lives at `talaria.getboxie.com`.

## Instance hosting domain (Admin → Org)

Where this deployment lives. Verification is a **self-fetch round trip**: the server requests its
own identity beacon (`/api/well-known/talaria-instance`) through the candidate domain and checks
the instance id that answers — proof that DNS, routing, and TLS all land on THIS deployment, not
just that you own the name. Once verified it becomes the canonical base URL: stable MCP OAuth
callbacks, invite links, share links.

## Invites (Admin → People)

Invite an email address → they receive a branded email with a `/join` link → the public join page
shows who invited them, to which org, and which address the invite is bound to → they continue with
Google and they're in; the invite stamps accepted.

- 14-day expiry; one live invite per address (re-inviting re-issues a fresh link and retires older
  pending ones); revoke kills a link instantly. State chips: pending / accepted / revoked / expired.
- Invite creation **survives email failure** — the invite persists and the panel surfaces the
  provider error, so a broken mail config never eats an invite (copy the link out of the email
  later, or fix mail and re-invite).

## Transactional email (Admin → Org → Email)

One `sendEmail` seam every feature uses (invites today, more later). Two providers:

- **Your own SMTP** — any server; Google Workspace works with `smtp.gmail.com`, port 587, an app
  password.
- **Resend** — an API key.

Secrets are sealed (envelope-encrypted) at rest and never echoed back — the panel shows set-flags
only. "Send me a test" delivers to the signed-in admin. Config writes are audited.

More providers land as requested; the seam is provider-shaped, not Resend-shaped.
