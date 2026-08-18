// Reading a MEMORY.md as the list of things it remembers.
//
// SEPARATE FROM memory.ts ON PURPOSE, and not for tidiness: that module imports
// @tanstack/svelte-query, which drags .svelte files into any importer — and
// vitest runs in node, so a test touching it fails to load before it asserts
// anything. Pure logic that deserves tests has to live where the tests can
// reach it. This file imports nothing.

/** One remembered thing, as the library shows it. */
export interface MemoryEntry {
  /** Stable within a parse — the index, since two entries may share a title. */
  id: string
  title: string
  /** The markdown for just this entry, heading included. */
  body: string
}

/**
 * A MEMORY.md, split into the things it remembers.
 *
 * WHY SPLIT AT ALL. The file is one long document and the panel rendered it as
 * one long scroll, which is fine at ten lines and unusable at two hundred —
 * and an agent that has been running for a month has two hundred. The library
 * turns it into something you can scan.
 *
 * THIS IS A READ-SIDE VIEW ONLY. Editing still replaces the whole file through
 * the editor, and appending still appends, so nothing here has to round-trip:
 * a parse that loses a nuance costs a slightly odd picker row, never a
 * corrupted memory. That is the whole reason the split lives on this side of
 * the boundary rather than in the writer.
 *
 * Two shapes, because the file genuinely has two. An agent curating prose
 * writes headings; the quick-add field appends bare `- fact` lines, and a
 * document made entirely of those has no headings to split on.
 */
export function parseMemory(content: string): MemoryEntry[] {
  const text = content.replace(/\r\n/g, '\n').trim()
  if (!text) return []

  const lines = text.split('\n')
  const headingAt = (l: string) => /^#{1,6}\s+\S/.test(l)

  if (lines.some(headingAt)) {
    const out: MemoryEntry[] = []
    let title: string | null = null
    let buf: string[] = []
    const flush = () => {
      const body = buf.join('\n').trim()
      // Preamble before the first heading counts only if it says something.
      if (title === null && !body) return
      out.push({ id: String(out.length), title: title ?? 'Preamble', body })
      buf = []
    }
    for (const line of lines) {
      if (headingAt(line)) {
        flush()
        title = line.replace(/^#{1,6}\s+/, '').trim()
        buf = [line]
      } else {
        buf.push(line)
      }
    }
    flush()
    return out
  }

  // No headings: every top-level bullet is one remembered fact. Continuation
  // lines (indented, or plain prose under a bullet) belong to the bullet above.
  const out: MemoryEntry[] = []
  let buf: string[] = []
  const flushBullet = () => {
    const body = buf.join('\n').trim()
    if (!body) return
    out.push({ id: String(out.length), title: bulletTitle(body), body })
    buf = []
  }
  for (const line of lines) {
    if (/^[-*+]\s+\S/.test(line)) flushBullet()
    buf.push(line)
  }
  flushBullet()
  return out
}

/** The picker row's label for a bullet: its first line, without the marker and
 *  without the "(added by hand, …)" stamp, which is provenance rather than
 *  content and would push the fact out of a truncated row. */
function bulletTitle(body: string): string {
  const first = body.split('\n')[0] ?? ''
  return (
    first
      .replace(/^[-*+]\s+/, '')
      .replace(/\s*_\(added by hand,[^)]*\)_\s*$/, '')
      .replace(/^[#*_`]+|[*_`]+$/g, '')
      .trim() || 'Untitled'
  )
}
