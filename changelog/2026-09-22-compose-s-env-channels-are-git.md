- **Compose's env channels are git-ignored**: `compose.env`, `compose.override.yml` (now the
  devbox carrier for provider tokens and `--env` secrets) and `docker-compose.override.yml`, as
  bare patterns so a devbox tree relocated into a checkout (`TALARIA_DEVBOX_HOME`) is covered
  too. A regression test pins the canonical secret-carrying paths to `git check-ignore`.
