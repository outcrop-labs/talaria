// Shared bits of the Artifacts page (Artifacts.svelte + tree rows + editor).
import { FileText, Table, Globe2, Paperclip, type LucideIcon as IconType } from '@lucide/svelte'
import type { ArtifactKind } from '@/lib/artifacts'

export const KIND_ICON: Record<ArtifactKind, IconType> = { doc: FileText, sheet: Table, microsite: Globe2, file: Paperclip }

// Artifacts — versioned work products with their own hosting + sharing. This
// foundation covers the doc kind (markdown); sheets, microsites, and files, plus
// cloud-storage connectors and the "make official → knowledgebase" pipeline, are
// tracked follow-ups.
export const NEW_KINDS: { kind: ArtifactKind; label: string; icon: IconType }[] = [
  { kind: 'doc', label: 'Document', icon: FileText },
  { kind: 'microsite', label: 'Microsite', icon: Globe2 },
  { kind: 'file', label: 'File upload', icon: Paperclip },
]

export type Drag = { kind: 'artifact' | 'folder'; id: string } | null

// A spreadsheet-style grid. The sheet body is JSON `string[][]` — row 0 is the
// header row. Edit mode is an editable grid with add/delete row+column; read
// mode renders a table. Autosaves (debounced).
export function parseGrid(body: string): string[][] {
  try {
    const g = JSON.parse(body)
    if (Array.isArray(g) && g.length && g.every((r) => Array.isArray(r))) return g.map((r: unknown[]) => r.map((c) => String(c ?? '')))
  } catch {
    /* fall through to a starter grid */
  }
  return [['Column A', 'Column B', 'Column C'], ['', '', ''], ['', '', '']]
}
