// Shared types for the session rail (see ConversationSidebar.svelte).

/** A read that FAILED, handed down from the route that owns the query. This
 *  component only ever sees already-defaulted arrays (`= []`), so an outage and
 *  a genuinely empty list arrive here looking exactly alike — the caller has to
 *  tell it which one it is, or "No sessions yet." goes out over a
 *  500 and a person's plans read as never having existed. */
export type SidebarFailure = { error: unknown; retry: () => void } | null
