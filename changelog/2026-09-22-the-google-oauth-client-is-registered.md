- **The Google OAuth client is registered in the Admin UI, not a .env file.**
  Admin → Org → Google Workspace · OAuth client: paste the client ID, secret,
  and an optional Workspace domain restriction; the secret is sealed
  (AES-256-GCM) in the database and never shown again. The panel lists the
  exact redirect URIs to authorize in Google Cloud Console — account connect,
  org connect, and login — with copy buttons, which was the step the .env
  workflow never helped with. The env vars remain as a fallback for scripted
  deployments; the Admin-UI record wins when one exists, and removing it hands
  control back to the env. Google LOGIN stays env-flag-gated
  (AUTH_GOOGLE_ENABLED) on purpose — registering a workspace client must not
  silently open a new way into the instance — but the flag now works with
  credentials from either source.
