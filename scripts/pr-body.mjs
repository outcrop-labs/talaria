// pr-body — read the `## ` sections of a pull request body, and tell an answer
// apart from the template's own scaffold. judge-pr.mjs asks "is the Verified
// section filled in?"; this file is how that question gets a true answer.
//
// WHY A LINE WALK AND NOT ONE REGEX. The reader this replaced was
// `^##\s*Name\s*$([\s\S]*?)(?=^##\s|\s*$)` with the `m` flag. `$` matches at
// every line end under `m`, so the lazy body stopped at the first one — the
// end of the heading line — and every section read as "". A walk over lines,
// cutting at the next level-2 heading outside a code fence, has no such edge.
//
// WHY "FILLED" IS MEASURED AGAINST THE TEMPLATE. .github/PULL_REQUEST_TEMPLATE.md
// does not leave a section empty for the author: under `## Verified` it carries
// the `- **Gate:**` / `- **Exercised:**` labels, two paragraphs about the gate and
// the judge, and a code block. A body opened from the template and never
// touched is a thousand characters long there. So an answer is what the
// section holds beyond the template's lines for the same section, with label
// bullets that carry no text after the label dropped as well.
//
// GitHub web edits send `\r\n`; HTML comments are prompts, not answers, and a
// heading inside one does not render. Both are normalized away first.

/** A fence line: the run of backticks or tildes, and what follows it. An open
 *  fence closes only on a run of the same character at least as long, with
 *  nothing after it (CommonMark). Inside a fence, a line that opens another
 *  one (three backticks and `bash`) is text, not the end. */
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/
/** A level-2 ATX heading as GitHub renders one: `##`, then a space or the end
 *  of the line (`##Verified` is a paragraph), an optional closing `##`. */
const H2 = /^ {0,3}##(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/
/** `- **Gate:**` (or `- **Gate**:`) with nothing after it — a label the
 *  author has not answered. A bullet that is only a bold sentence is an answer. */
const EMPTY_LABEL = /^\s*[-*+]\s+\*\*[^*]+?(?::\*\*|\*\*:)\s*$/

function normalize(markdown) {
  return String(markdown ?? '')
    .replace(/\r\n?/g, '\n')
    .replace(/<!--[\s\S]*?-->/g, '')
}

/** Every level-2 section: lowercased heading → the lines under it, untrimmed.
 *  The first occurrence of a heading wins, the way a reader would take it. */
export function sections(markdown) {
  const out = new Map()
  let current = null
  let fence = null
  for (const line of normalize(markdown).split('\n')) {
    const f = FENCE.exec(line)
    if (f) {
      const [, run, rest] = f
      if (fence === null) fence = run
      else if (run[0] === fence[0] && run.length >= fence.length && rest.trim() === '') fence = null
    }
    const h = fence === null && !f ? H2.exec(line) : null
    if (h) {
      const key = (h[1] ?? '').trim().toLowerCase()
      current = out.has(key) ? null : []
      if (current) out.set(key, current)
      continue
    }
    if (current) current.push(line)
  }
  return out
}

/** The text under `## <name>`, trimmed, or null when the body has no such
 *  heading. Case-insensitive on the name. */
export function section(markdown, name) {
  const lines = sections(markdown).get(String(name).trim().toLowerCase())
  return lines ? lines.join('\n').trim() : null
}

/** What the author wrote under `## <name>`: the section minus every line the
 *  template carries in its own section of that name, minus unanswered label
 *  bullets. null when the body has no such heading; "" when the heading is
 *  there and nothing was added to it. */
export function answer(markdown, name, template = '') {
  const lines = sections(markdown).get(String(name).trim().toLowerCase())
  if (!lines) return null
  const scaffold = new Set(
    (sections(template).get(String(name).trim().toLowerCase()) ?? [])
      .map((l) => l.trim())
      .filter(Boolean),
  )
  return lines
    .filter((l) => !scaffold.has(l.trim()) && !EMPTY_LABEL.test(l))
    .join('\n')
    .trim()
}
