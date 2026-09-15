// TALA-4: the pure half of the agent-refine notice — deciding WHEN a document
// update counts as an agent refine worth announcing, and summarizing WHAT
// changed. The component (AgentRefineNotice.svelte) owns the effects and the
// DOM; these functions own the decisions.

import { diffLines } from '@/components/fleet/line-diff'

export interface RefineRev {
  id: string
  createdBy: string | null
  createdAt: string
  size: number
}

/** Which revision deserves the announcement, if any: the newest one, when it
 *  was written by someone other than the local viewer and hasn't been seen.
 *  The viewer's own saves are never refines — they were already on screen
 *  when they happened. */
export function refineToAnnounce(
  revisions: RefineRev[],
  who: string | null,
  seenAt: string | null,
): RefineRev | null {
  const latest = revisions[0]
  if (!latest) return null
  if (who !== null && latest.createdBy === who) return null
  if (seenAt === latest.createdAt) return null
  return latest
}

/** The identity a save is stamped with, mirroring the API's actor rule
 *  (`who_of`: email first, name as fallback). The viewer's own revision is
 *  recognized by exactly this string. */
export function viewerStamp(me: { email: string | null; name: string | null } | null): string | null {
  return me?.email ?? me?.name ?? null
}

export interface RefineSummary {
  adds: number
  dels: number
  /** Line-diff sized the document: the summary is trustworthy. */
  text: string
  /** The document was too large to diff — say "changed" without numbers. */
  oversized: boolean
}

/** What changed, for the notice line: "3 added · 1 removed line". Degrades to
 *  an honest "changed" when the document outgrew the client diff. */
export function summarize(before: string, after: string): RefineSummary {
  const d = diffLines(before, after)
  if (!d) return { adds: 0, dels: 0, text: 'changed', oversized: true }
  const adds = d.filter((l) => l.type === 'add').length
  const dels = d.filter((l) => l.type === 'del').length
  const lineWord = adds === 1 && dels === 0 ? 'line' : 'lines'
  if (adds === 0 && dels === 0) return { adds, dels, text: 'no text changes', oversized: false }
  return { adds, dels, text: `${adds} added · ${dels} removed ${lineWord}`, oversized: false }
}

/** Snapshots store `# Title\n\n<body>`; the document surfaces show the body.
 *  Split them back out — or pass the content through when it isn't that shape
 *  (a first body without a title, a space landing page). */
export function snapshotBody(content: string): string {
  const m = /^#\s(.*)\n+([\s\S]*)$/.exec(content)
  return m ? m[2]! : content
}
