- **The release pipeline ran for the first time, and the nightly feed came back
  to life.** No `v*` tag had ever existed here, and every scheduled nightly for
  at least the eleven runs before 2026-09-16 had died as a `startup_failure` —
  a run with no jobs and no logs, which is why the channel could be dead for
  eleven days without anyone noticing. Cutting `v0.1.0-rc.1` surfaced three
  defects that only a real run could: **(1)** a called workflow's jobs may
  request no more than the CALLING JOB grants, so `release.yml`'s nested
  `api-package` call was refused for asking `packages: write` under a
  `contents: read` caller — the grants now sit on the calls, not raised
  workflow-wide; **(2)** the installers' `attach` job ran `gh release upload`
  without ever checking the repository out, and `gh` resolves the repository
  from a local clone, so the first run stopped at the very last step
  (`failed to run git: not a git repository`) with every artifact correct and
  in hand; **(3)** `testing`, the ref a nightly builds, was three weeks stale
  (a 2026-08-26 tree with no `api/` and no `desktop/`), so the first nightly
  that got as far as building found directories that did not exist. All three
  are fixed, the channel was advanced, and `v0.1.0-rc.1` published: GHCR
  `0.1.0-rc.1` + `rc` for both the app image and the api package, a GitHub
  prerelease carrying every installer plus a `SHA256SUMS`
  ([release](https://github.com/outcrop-labs/talaria/actions/runs/35111577866)),
  and the first working nightly in the channel's recorded history —
  `nightly` + `nightly-YYYYMMDD`
  ([nightly](https://github.com/outcrop-labs/talaria/actions/runs/35111665940)).
  `RELEASING.md` now names the two ways a nightly stops for good: the 60-day
  schedule rule, and a startup failure whose reason lives only in the run page's
  banner. A second desktop-installer workflow, merged from a parallel session
  after that release (triggering on the same `v*` tags, uploading to the same
  release with `--clobber`, and asking Tauri for a `flatpak` bundle type it does
  not have), was removed in favour of the one `release.yml` actually calls: one
  publisher per tag.
