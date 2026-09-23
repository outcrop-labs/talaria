- **Invites + transactional email.** The third admission door: invite an
  email address, they get a branded join link (public /join page shows
  who invited them, to which org, bound to which address), and signing in
  with Google admits them, stamping the invite accepted. 14-day expiry,
  one live invite per address, instant revoke, state chips. Email rides a
  provider seam — your own SMTP (Google Workspace works with an app
  password) or Resend — with sealed secrets, masked GETs, and a
  send-me-a-test button. Invite creation survives a broken mail config
  (the error surfaces; the invite persists).
