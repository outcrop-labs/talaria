// The row and table grammar the Observability tabs share.

/** The caption-style table: sentence-case headers, 44px rows, numbers
 *  right-aligned in tabular figures, identifiers in mono. */
export const table = {
  root: 'w-full border-collapse font-sans text-sm leading-5 text-fg',
  th: 'h-10 whitespace-nowrap px-3 text-left font-sans text-xs font-medium leading-4 text-muted first:pl-5 last:pr-5',
  tr: 'h-11 border-t border-line-subtle',
  td: 'px-3 first:pl-5 last:pr-5',
  num: 'whitespace-nowrap text-right tabular-nums',
  id: 'font-mono text-xs',
} as const

/** A link from a section's title row to the tab or page that owns the detail. */
export const sectionLink =
  'flex items-center gap-1 rounded font-sans text-[13px] font-medium leading-5 text-muted transition-colors hover:text-fg focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent'
