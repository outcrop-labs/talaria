// A document's leading YAML block, lifted out of the text a rich editor edits
// and put back when the text is read. `head + gap + body` is the document,
// byte for byte, so a save that never touched the block writes it back as it
// was.

export interface FrontmatterParts {
  /** The block, from the opening fence through the closing one. Empty when
   *  the document has none. */
  head: string
  /** The line breaks between the block and the body. */
  gap: string
  body: string
}

// A fence is a whole line: `---`, then at most spaces or tabs. A line that
// only starts with three dashes (a rule drawn longer, a wrapped quoted value)
// does not close the block.
const FRONTMATTER = /^(---[ \t]*\r?\n(?:[\s\S]*?\r?\n)?---[ \t]*)(?![^\r\n])((?:\r?\n)*)/

export function splitFrontmatter(text: string): FrontmatterParts {
  const m = FRONTMATTER.exec(text)
  if (!m) return { head: '', gap: '', body: text }
  const head = m[1] ?? ''
  const gap = m[2] ?? ''
  return { head, gap, body: text.slice(head.length + gap.length) }
}

/** The document again. `editedHead` is the block as someone retyped it in a
 *  text field, which hands back `\n` line breaks whatever the document used. */
export function joinFrontmatter(parts: FrontmatterParts, editedHead?: string): string {
  const eol = parts.head.includes('\r\n') ? '\r\n' : '\n'
  const head = editedHead === undefined ? parts.head : editedHead.trimEnd().replace(/\r?\n/g, eol)
  if (!head.trim()) return parts.body
  // A block that ended the document had no break after it. A body typed
  // under it needs one, or its first line joins the closing fence.
  const gap = parts.gap || (parts.body ? eol : '')
  return `${head}${gap}${parts.body}`
}
