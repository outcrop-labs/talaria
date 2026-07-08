# Google Workspace integration

Per-user OAuth: each person connects **their own** Google account, and Talaria —
plus the agents working for them — acts *as that user* against Drive/Docs. Files
land in the user's own Drive, governed by Google's own sharing. Talaria never
uses a shared service account for this.

## What ships today

- **Connect / disconnect** a Google account from **Settings → Connected
  accounts**. Connecting grants *offline* access (a refresh token) so exports
  work later without the user present.
- **Export an artifact → Google Drive** from an artifact's **⋯ menu**:
  - `doc` → native **Google Doc** (markdown converted by Drive)
  - `microsite` → native **Google Doc** (HTML converted by Drive)
  - `sheet` → native **Google Sheet** (CSV converted by Drive)
  - `file` → uploaded to Drive unconverted (original type preserved)
  - The resulting Drive link is stored on the artifact ("Open in Google Drive").
- **Agents** export via the `export_to_google_doc` MCP tool. An agent has no
  Google account, so it exports into the artifact **owner's** Drive — the human
  it works for must be connected. (Identity proxy, #42.)

## Security

- Refresh tokens are **encrypted at rest** (AES-256-GCM, `server/secretbox.ts`,
  key derived from `AUTH_SECRET`). They are the one runtime credential we must
  store — unlike API keys, where we persist only the env-var *name*.
- Access tokens are cached (also encrypted) and refreshed transparently; an
  `invalid_grant` clears the connection so the UI prompts a reconnect.
- Scope is least-privilege: **`drive.file`** — access only to files the app
  itself creates, not the user's whole Drive.
- Disconnect best-effort **revokes** the token at Google, then forgets it.

## Operator setup

Uses the **same** Google OAuth client as Google login:

```
AUTH_GOOGLE_ENABLED=1
AUTH_GOOGLE_CLIENT_ID=<client id>.apps.googleusercontent.com
AUTH_GOOGLE_CLIENT_SECRET=<client secret>
AUTH_SECRET=<random, ≥32 bytes>        # also encrypts stored tokens
# AUTH_GOOGLE_HD=yourdomain.com        # optional: restrict to a Workspace domain
# TALARIA_SECRET_KEY=<override>         # optional: separate key for secretbox
```

In the **Google Cloud console** for that OAuth client, register **both** redirect
URIs (login + connect):

```
<public-url>/api/auth/google/callback
<public-url>/api/integrations/google/callback
```

and enable the **Google Drive API**. Add the `.../auth/drive.file` scope to the
consent screen. `drive.file` is not a restricted scope, so it needs no Google
verification review for internal/Workspace apps.

## Verified vs. trusted

The connect redirect (offline + `drive.file` + `prompt=consent`), the token
encryption round-trip/tamper-rejection, the DB model, and the export permission
gating are all verified live. The **actual Drive round-trip** (create Doc/Sheet)
requires real Google client credentials + a consented account and cannot be
exercised in a headless dev env — it's wired to Drive's documented multipart
convert-on-upload API but is the one path proven by the API contract rather than
a local screenshot.
