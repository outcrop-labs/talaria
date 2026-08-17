// Which side of the MCP page a server belongs on.
//
// In `lib/` rather than beside the page because `src/routes/**` is excluded
// from the test run (see the note in vitest.config.ts: keep route files thin,
// put the logic where it can be tested). The split is not cosmetic — the
// External tab is the only one offering "add" and "repoint", because those are
// the only servers where either means anything.

/** The fields the decision reads. Structural so `lib/` needs no import from a
 *  route module. */
export interface ServerOrigin {
  builtin: boolean
  appSlug: string | null
  url: string
}

/** Internal servers are served from inside this deployment and their lifecycle
 *  belongs to the platform: Talaria's own toolkit (`builtin`), the Workbench
 *  surface (a `talaria-workbench://` routing token rather than a hostname), and
 *  anything an installed app publishes (`appSlug`). You govern WHO may use
 *  them; you do not register or point them anywhere.
 *
 *  External servers are third-party endpoints somebody registered — a URL, a
 *  credential and a trust decision.
 *
 *  The SCHEME test is what catches the Workbench, which is in-process like an
 *  app server but carries neither flag. Testing the scheme rather than matching
 *  "talaria" anywhere in the URL matters: a third-party host is free to call
 *  itself talaria-workbench.evil.example, and substring matching would have
 *  filed it under our own tools. */
export const isInternalServer = (s: ServerOrigin): boolean =>
  s.builtin || !!s.appSlug || !/^https?:\/\//i.test(s.url)
