- **W4b — one format module.** `ui/src/lib/format.ts` owns `formatUsd`
  (`{ unpricedAsWord }`), `formatBytes` (`{ ceilKb }`), `formatTokens` and
  `fmtDuration`. The flags are real conventions, not rounding preferences: the
  two byte cascades also disagree on non-finite input (`NaN MB` vs `NaN KB`) and
  past a GiB (`1024.0 MB` vs `1.0 GB`), so merging them would have changed those
  strings. Verified by a throwaway parity script: 89 value comparisons across
  the six helpers, 0 mismatches (0, a boundary, negatives, NaN, Infinity, null),
  plus `fitness.test.ts`'s four pins. `formatCost`/`formatTokens`/`humanSize`/
  `fmtTime` each remain one-line re-exports so unowned call sites keep working;
  `TaskDetail.svelte`'s own call sites were renamed.
