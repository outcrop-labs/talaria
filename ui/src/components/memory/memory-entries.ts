// MEMORY.md AS THE LIST OF THINGS REMEMBERED — the entry model, and nothing else.
//
// Hermes keeps a memory file as a §-DELIMITED LIST: entries joined by
// `\n§\n`. Its own memory tool re-reads the file before it writes and REFUSES
// every write once the file stops round-tripping as a clean list — so a hand
// edit that leaves a stray marker, a blank entry, or CRLF line endings does
// not cost a cosmetic glitch, it silences the agent's own memory until a
// person repairs the file. Every function here exists so that what the UI
// writes back is exactly the shape Hermes expects, whatever shape it found.
//
// PURE, AND SEPARATE FROM `memory.ts` ON PURPOSE — not for tidiness: that
// module imports @tanstack/svelte-query, which drags .svelte files into any
// importer, and vitest runs in node, so a test touching it fails to load
// before it asserts anything. This file imports nothing.

/** The marker a delimiter line carries, and nothing else. */
const MARKER = '§'

/** The exact separator the file uses between two entries. */
const DELIMITER = `\n${MARKER}\n`

/** Titles are one line in a card header, not a paragraph. */
const TITLE_MAX = 80

const lf = (s: string) => s.replace(/\r\n?/g, '\n')

/**
 * A memory file, split into its entries.
 *
 * THE DELIMITER IS A LINE, not a character. Only a line that is the marker
 * alone separates two entries (`\n§\n`, as Hermes writes it); a `§` with
 * anything else on its line is text — entries cite sections ("see §4 of the
 * policy"), and splitting on those would cut one entry in half and then write
 * the halves back as two.
 *
 * Tolerant in the directions that cost nothing and protect the file: a marker
 * line with stray whitespace still separates (a hand edit through the
 * whole-file editor leaves trailing spaces behind), CRLF normalises to LF,
 * blank lines around a delimiter are trimmed instead of becoming an entry,
 * and anything that trims to nothing is dropped — an empty entry is not
 * something somebody remembered, and writing one back is precisely the
 * malformed list the agent refuses to work with.
 */
export function splitEntries(content: string): string[] {
  const out: string[] = []
  let buf: string[] = []
  const flush = () => {
    const text = buf.join('\n').trim()
    if (text) out.push(text)
    buf = []
  }
  for (const line of lf(content).split('\n')) {
    if (line.trim() === MARKER) flush()
    else buf.push(line)
  }
  flush()
  return out
}

/**
 * The entries, back as the file on disk.
 *
 * The inverse of `splitEntries` for anything already clean, and the
 * NORMALISER for anything that was not: each entry trimmed, empties dropped,
 * one marker line between neighbours. `trailingNewline` keeps the ending the
 * file had — the agent writes one, so it is the default, and a caller that
 * read a file without one passes `false` rather than inventing a diff nobody
 * made.
 */
/** A line inside an entry that is exactly the marker would split the entry
 *  on the next read — the one silent edit this module exists to prevent — so
 *  it is written escaped. Hermes reads the file as prose; a backslash is
 *  text to it and to the next `splitEntries`. */
const neutralizeMarkerLines = (entry: string): string =>
  entry
    .split('\n')
    .map((line) => (line.trim() === MARKER ? `\\${line}` : line))
    .join('\n')

export function joinEntries(entries: string[], trailingNewline = true): string {
  const clean = entries.map((e) => neutralizeMarkerLines(lf(e).trim())).filter(Boolean)
  if (!clean.length) return ''
  return `${clean.join(DELIMITER)}${trailingNewline ? '\n' : ''}`
}

/**
 * The INDICES of the entries matching a query — indices, not entries, because
 * every write addresses an entry by its position in the list and a filtered
 * copy would hand the caller positions that are wrong by exactly the number
 * of entries the filter hid.
 *
 * Plain case-insensitive substring over the whole entry: the file is prose a
 * person skims, and one box that finds the word they remember beats a query
 * language nobody is going to learn here. A blank query matches everything.
 */
export function searchEntries(entries: string[], query: string): number[] {
  const q = query.trim().toLowerCase()
  const all = entries.map((_, i) => i)
  // `?? ''` is the index-safety the tsconfig asks for, not a real case:
  // every index here came from this same list.
  return q ? all.filter((i) => (entries[i] ?? '').toLowerCase().includes(q)) : all
}

/**
 * The one line that names an entry in the list.
 *
 * Markdown furniture is not a title: the agent writes `## Preferences`
 * headings and the quick-add line appends bare `- fact` rows, and a card
 * header reads better without the markers.
 */
export function entryTitle(entry: string): string {
  const first =
    lf(entry)
      .split('\n')
      .map((l) => l.trim())
      .find(Boolean) ?? ''
  const text = first
    .replace(/^#{1,6}\s+/, '')
    .replace(/^[-*+]\s+/, '')
    .trim()
  if (text.length <= TITLE_MAX) return text
  // The one tolerated ellipsis (UI-CONVENTIONS: a truncation marker on
  // clipped USER text). It is counted INSIDE the budget, so a title is never
  // wider than the budget by the width of its own marker.
  return `${text.slice(0, TITLE_MAX - 1).trimEnd()}…`
}

/** One entry replaced, as a new list. Editing an entry down to nothing is a
 *  REMOVAL — the alternative is a blank entry in the list, which is the shape
 *  that stops the agent writing. An index nothing lives at changes nothing. */
export function replaceEntry(entries: string[], index: number, text: string): string[] {
  if (index < 0 || index >= entries.length) return [...entries]
  const next = lf(text).trim()
  return next ? entries.map((e, i) => (i === index ? next : e)) : removeEntry(entries, index)
}

/** One entry dropped, as a new list. An index nothing lives at changes nothing. */
export function removeEntry(entries: string[], index: number): string[] {
  return entries.filter((_, i) => i !== index)
}
