# pr-watch fixtures

Recorded `gh` output that [`../pr-watch.test.mjs`](../pr-watch.test.mjs) runs the watcher's
verdict against. They are what GitHub returned, not hand-written: the test derives the
in-flight states (a partial rollup, a run still going, a queued re-run) from these by
removing or rewriting entries, and says so at each case.

| File | Recorded with | What it holds |
|---|---|---|
| `pr-599.json` | `gh pr view 599 --json <FIELDS>` | A body edit right after `gh pr create` re-ran `judge`: a CANCELLED `judge (standards)` beside the newer SUCCESS one on the same head, and a fully green CI. |
| `runs-599.json` | `gh run list --commit <head> --json <RUN_FIELDS>` | The same head's workflow runs, the cancelled judge run included. |
| `pr-575.json` | `gh pr view 575 --json <FIELDS>` | CI's api, ui and end-to-end jobs failed on the head. |
| `runs-575.json` | `gh run list --commit <head> --json <RUN_FIELDS>` | The same head's runs: CI concluded `failure`. |

`<FIELDS>` and `<RUN_FIELDS>` are the constants in [`../pr-watch.mjs`](../pr-watch.mjs). Both
pull requests are merged, so GitHub now reports them `mergeable: UNKNOWN`; the test's `asOpen`
puts them back in the open, mergeable state they were in while CI ran.

To record another: run the two commands above for the pull request and its `headRefOid`, save
them here, and name the case in the test after what the recording shows.
