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
| `pr-607-t4.json`, `runs-607-t4.json` | both commands, 4 s after `gh pr create` for #607 | Two runs queued (`judge`, `CI`), and no check in the rollup yet. |
| `pr-607-t17.json`, `runs-607-t17.json` | the same, 17 s after | CI's skipped surface jobs registered; `judge (standards)` and `invariants (no deps)` in progress; the CI run in progress. |
| `pr-607-t21.json`, `runs-607-t21.json` | the same, 21 s after | Every run finished on the head: a real open, mergeable green. |
| `pr-597.json`, `runs-597.json` | both commands, for the promotion #597 (`rc` → `main`) on 2026-11-02 | The promotion of `rc` at `678dd627`: open, MERGEABLE, BLOCKED on review, and every check finished. CI and flow ran for both the push to `rc` and the pull request, so their jobs are listed twice, several as SKIPPED beside SUCCESS. |

`<FIELDS>` and `<RUN_FIELDS>` are the constants in [`../pr-watch.mjs`](../pr-watch.mjs). #599
and #575 are merged, so GitHub now reports them `mergeable: UNKNOWN`; the test's `asOpen` puts
them back in the open, mergeable state they were in while CI ran. The #607 and #597 recordings were
taken while they were open and are used as recorded.

To record another: run the two commands above for the pull request and its `headRefOid`, save
them here, and name the case in the test after what the recording shows.

The `review-*` and `offload-review-check.json` fixtures are SYNTHESISED in GitHub's shapes
(check-run annotations, issue comments with `--slurp` pages, rollup check runs) for the
`judge (review)` reading; they are replaced by recordings after the first calibration run.
