- **Desktop updates itself from a GitHub Release.** Settings → Profile (and
  the launcher) Check for updates reads `/releases/latest/download/latest.json`,
  verifies a minisign signature, replaces the install, and relaunches. Stable
  tags attach `latest.json` plus `.sig` files for the AppImage, the universal
  `.app.tar.gz`, and the NSIS installer; RCs do not (GitHub's `/releases/latest`
  is the stable pointer). Verified: `write-latest-json.py --self-test`; cargo
  test + clippy on the new `check_for_update` / `install_update` commands.
