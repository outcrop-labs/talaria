- **A desktop release opens an Omapak submission.** A published stable
  release (`desktop-v*` or a suffix-free `v*`) runs `omapak-submit.yml`,
  which refreshes `app.talaria.desktop` — tag, commit, vendored cargo and
  bun sources, rust and bun dist pins — and opens a pull request on
  outcrop-labs/omapak. The judge still scores it; nothing merges from the
  workflow. Needs `OMAPAK_SUBMIT_TOKEN`. Verified: `node
  scripts/omapak-desktop-submit.mjs --self-test` parses the desktop
  lockfiles and matches a known crate checksum and the published
  `@esbuild/linux-x64` sha512.
