- **Desktop packaging copy says what the app is.** The Flatpak/AppStream
  listing, `.desktop` comment, pacman `pkgdesc`, and installer descriptions
  call this the official Talaria desktop client, describe Talaria in the
  same voice as talariaworks.ai (one workspace, agents as teammates), and
  point homepage at https://talariaworks.ai. `stage.sh` now ships the
  metainfo into the FHS tree both packagers consume. Verified:
  `desktop-file-validate`; `appstreamcli validate --no-net`; `bun run check`.
