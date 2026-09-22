- **First-run claim + admin-managed auth.** A fresh instance offers `/claim` (email + password,
  plus "Claim with Google" when Google login is on); the first identity through becomes the
  admin, advisory-lock serialized so a race can't mint two. From then on auth is managed in the
  app: Admin → People gains **password accounts** (create, reset, remove — including your own),
  roles live only in the database (a sign-in never changes one; the last admin can't be demoted),
  and once a Google client is configured, Admin → Google client links the admin's own Google
  account.
- Google login can be enabled from the Admin UI (Admin → Google client): the toggle writes the
  `google_login_enabled` setting (`PUT /api/admin/google-client/login`), so login no longer needs
  `AUTH_GOOGLE_ENABLED=1` in the env. The env var still pins login ON (undeactivatable from the
  UI); the toggle stays inert until a client is actually configured.
- `talaria service` — `install` starts the production stack (the `deploy up` build, in your
  terminal) and installs a systemd unit that starts it at boot, health-gates that start on the
  compose healthchecks (`up -d --wait`, dropped automatically on compose builds that hang on it),
  and stops it cleanly (`compose down`) before docker.service; `uninstall` removes both, keeping
  volumes and state; `status` shows the unit state and the compose view. `install` also pins
  `DOCKER_GID` into `docker/.env` — the boot unit has no shell to resolve it from
  (docs/CONTAINER.md → "Keep it running across reboots").

### Changed
