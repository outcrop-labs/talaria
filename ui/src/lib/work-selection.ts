// WHICH SESSION THE WORK HARNESS HAS OPEN — read off the URL, and nothing else.
//
// The harness answers at three addresses (see router.ts): `/work/<id>` is a
// session, `/plan/<id>` is that same row at the address it was linked and
// bookmarked under before Work, Plan and Research became one surface, and
// `/research/<runId>` names a RUN rather than a conversation — the harness
// resolves the run's own discussion and replaces the address with
// `/work/<conversationId>`.
//
// Three addresses, two reads, and the whole point is which address feeds which
// read. A run id is NOT a conversation id, so a `/research/<runId>` that leaked
// into the session selection would hand the conversation plane an id it has
// never seen — a 404 rendered as "this session is gone" for the beat before the
// redirect lands, on the one path a mailed link is most likely to arrive by.
//
// It lives here rather than inline in WorkHarness.svelte so the rule can be
// tested at all: the component is a surface with a dozen queries behind it, and
// mounting it to ask "does /plan/<id> select a session" is a test of everything
// except the question. This file is PURE — a string in, a string or null out,
// and nothing in it knows Svelte exists.

import { pathId } from '@/lib/route-tabs'

/** The conversation the harness has selected, or null when it is open with
 *  nothing selected — a real state, and what the bare `/work` list is.
 *
 *  `/plan/<id>` reads as a selection because a plan IS a session now and its
 *  old address names the same row. New selections move the URL to `/work`; the
 *  old address keeps resolving, because a link somebody was mailed is a
 *  promise. */
export function sessionIdFor(pathname: string): string | null {
  return pathId(pathname, '/work') ?? pathId(pathname, '/plan')
}

/** The research run a legacy `/research/<runId>` address names, or null.
 *
 *  Separate from `sessionIdFor` on purpose, and not merged into it even though
 *  both are "the id in the URL": this one is a run, which the harness must
 *  trade for the run's conversation before anything conversation-shaped may be
 *  asked about it. The separation is what keeps a run id out of the session
 *  queries (see the note at the top of the file). */
export function legacyRunIdFor(pathname: string): string | null {
  return pathId(pathname, '/research')
}
