import type { BriefAbsent, BriefResponse } from './daily-brief-types'

// ONE DEFINITION, REACHABLE FROM BOTH SIDES.
//
// This predicate is needed by the browser (which branch of the surface to
// render) and by the server, and the obvious spellings are both wrong:
//
//   · exporting it from `@/lib/daily-brief-types` and importing the VALUE
//     into a component makes the client bundle depend on the server module
//     graph — the graph that reaches the database pool, the harness runner and
//     the guard registry. `scripts/check-invariants.mjs` fails the build on it,
//     and it caught exactly this import.
//   · declaring it twice, once per side, is what that rule offers as the way
//     out. It works, and it is duplication held together by a test.
//
// `@/lib` is the third option and the right one here, because this file needs
// no VALUE from the server at all — only shapes. `import type` is erased at
// build time, so nothing of the server graph survives into the bundle, and
// there is one definition rather than two that can drift. Same arrangement
// `server/digest.ts` already has with `@/lib/notify-classes`.
//
// The bar for putting something here is that it is pure and needs no runtime
// server import. A predicate over a discriminated union clears it; anything
// that has to read a row does not.

/** Is this "no brief" rather than a brief? */
export const isBriefAbsent = (value: BriefResponse): value is BriefAbsent => 'absent' in value
