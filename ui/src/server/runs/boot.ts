// THE WIRING, and it is the file the whole durability story rests on.
//
// WHAT IT IS FOR. `runs/reclaim.ts` registers the `run-reclaim` job at module
// load, and each run definition registers its kind the same way — but nothing
// in the server graph reliably imports either, and both failures are silent:
//
//   · no reclaim job  → a run whose driver died is never re-entered. Every
//     checkpoint is written correctly and read by nobody.
//   · no definition   → an instance that finds a due row of a kind it has never
//     imported leaves it alone, deliberately (`drive` returns `no-definition`).
//     If NO instance has imported it, the row is left alone by everybody,
//     forever, with no error anywhere.
//
// SO: ONE IMPORT LIST, HERE, AND ONE IMPORTER. `routes/api/runs.$id.events.ts`
// imports this module for its side effects, and `src/server/app.ts` eagerly
// globs `../routes/api/**` — the same mechanism every other job module in the
// tree uses. `run-reclaim` is in REQUIRED_JOBS, so an instance that boots
// without reaching this file fails its own boot check out loud rather than
// running without durability.
//
// NOTHING IS EXPORTED and nothing here runs a query: the cost of importing it
// is the cost of loading what it names, which any instance owes the moment it
// is asked to reclaim one.

// The sweep. Registers 'run-reclaim' with the scheduler at module load.
import './reclaim'

// THE KINDS GO HERE, one line per file in `defs/`, alphabetically — so a
// definition that is not on this list is a visible omission rather than an
// invisible one.
//
// Each kind arrives in its own change and adds its own line here, which is
// what makes those changes reviewable one subsystem at a time.
import './defs/reindex'
import './defs/research'
import './defs/work-session'
