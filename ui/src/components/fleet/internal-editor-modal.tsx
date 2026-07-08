import { useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { History, RotateCcw, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Modal } from '@/components/ui/modal'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { relativeTime } from '@/lib/fleet'
import { cn } from '@/lib/cn'

interface Revision {
  id: string
  createdBy: string | null
  createdAt: string
  size: number
}

// ── Line diff (LCS) — small, dependency-free, capped for huge documents ──────
type DiffLine = { type: 'same' | 'add' | 'del'; text: string } | { type: 'skip'; count: number }

function diffLines(oldText: string, newText: string): DiffLine[] | null {
  const a = oldText.split('\n')
  const b = newText.split('\n')
  const n = a.length
  const m = b.length
  if (n * m > 2_000_000) return null // too big to diff comfortably in the client
  const w = m + 1
  const dp = new Int32Array((n + 1) * w)
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i * w + j] = a[i] === b[j] ? dp[(i + 1) * w + j + 1]! + 1 : Math.max(dp[(i + 1) * w + j]!, dp[i * w + j + 1]!)
  const raw: Array<Exclude<DiffLine, { type: 'skip' }>> = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) raw.push({ type: 'same', text: a[i++]! }), j++
    else if (dp[(i + 1) * w + j]! >= dp[i * w + j + 1]!) raw.push({ type: 'del', text: a[i++]! })
    else raw.push({ type: 'add', text: b[j++]! })
  }
  while (i < n) raw.push({ type: 'del', text: a[i++]! })
  while (j < m) raw.push({ type: 'add', text: b[j++]! })

  // Collapse long unchanged runs to 3 lines of context on each side.
  const out: DiffLine[] = []
  let run: Array<Exclude<DiffLine, { type: 'skip' }>> = []
  const flush = (last: boolean) => {
    const keep = 3
    if (run.length > keep * 2 + 1) {
      const head = out.length === 0 ? [] : run.slice(0, keep)
      const tail = last ? [] : run.slice(-keep)
      out.push(...head, { type: 'skip', count: run.length - head.length - tail.length }, ...tail)
    } else out.push(...run)
    run = []
  }
  for (const line of raw) {
    if (line.type === 'same') run.push(line)
    else {
      flush(false)
      out.push(line)
    }
  }
  flush(true)
  return out
}

function DiffView({ diff, fallback }: { diff: DiffLine[] | null; fallback: string }) {
  if (!diff)
    return (
      <div className="h-full overflow-y-auto rounded-xl border border-line-subtle p-3">
        <p className="mb-2 text-xs text-muted">Too large to diff — showing the revision's full content.</p>
        <pre className="whitespace-pre-wrap font-[var(--font-mono)] text-xs text-fg">{fallback}</pre>
      </div>
    )
  const changed = diff.some((l) => l.type === 'add' || l.type === 'del')
  if (!changed)
    return (
      <div className="grid h-full place-items-center rounded-xl border border-line-subtle text-sm text-muted">
        Identical to the editor's current content.
      </div>
    )
  return (
    <div className="h-full overflow-y-auto rounded-xl border border-line-subtle py-1">
      {diff.map((l, idx) =>
        l.type === 'skip' ? (
          <div key={idx} className="px-3 py-1 text-center text-[11px] text-muted">
            ··· {l.count} unchanged line{l.count === 1 ? '' : 's'} ···
          </div>
        ) : (
          <div
            key={idx}
            className={cn(
              'whitespace-pre-wrap px-3 font-[var(--font-mono)] text-xs leading-5',
              l.type === 'add' && 'bg-[color-mix(in_srgb,var(--theme-success)_14%,transparent)] text-fg',
              l.type === 'del' && 'bg-[color-mix(in_srgb,var(--theme-danger)_12%,transparent)] text-muted',
              l.type === 'same' && 'text-muted',
            )}
          >
            <span className="mr-2 inline-block w-3 select-none text-muted">
              {l.type === 'add' ? '+' : l.type === 'del' ? '−' : ''}
            </span>
            {l.text || ' '}
          </div>
        ),
      )}
    </div>
  )
}

/** The workspace for an agent-internal document (skill / memory / soul-like
 *  markdown): a near-fullscreen surface — WYSIWYG editor filling the height,
 *  version history in a rail. Clicking a revision shows a DIFF against the
 *  editor's current content; loading one stages it in the editor (not saved
 *  until you Save), so a revert is itself reviewable and produces a revision. */
export function InternalEditorModal({
  open,
  onClose,
  title,
  subtitle,
  value,
  editable,
  saving,
  onSave,
  history,
  footerExtra,
}: {
  open: boolean
  onClose: () => void
  title: string
  subtitle?: string
  value: string
  editable: boolean
  saving?: boolean
  onSave: (markdown: string) => Promise<void> | void
  /** Query params for /api/history (kind + owner/name or id). Omit to hide history. */
  history?: Record<string, string>
  /** Extra control on the left of the footer (e.g. a delete button). */
  footerExtra?: React.ReactNode
}) {
  const ref = useRef<RichEditorHandle>(null)
  const [dirty, setDirty] = useState(false)
  const [seed, setSeed] = useState(0) // bump to remount the editor with new content
  const [current, setCurrent] = useState(value)
  const [showHistory, setShowHistory] = useState(true)
  const [diffing, setDiffing] = useState<{ rev: Revision; content: string; diff: DiffLine[] | null } | null>(null)

  const { data: revisions = [], refetch } = useQuery({
    queryKey: ['history', history],
    enabled: open && !!history,
    queryFn: async (): Promise<Revision[]> => {
      const qs = new URLSearchParams(history).toString()
      const r = await fetch(`/api/history?${qs}`)
      if (!r.ok) return []
      return ((await r.json()) as { revisions: Revision[] }).revisions
    },
  })

  const save = async () => {
    const md = ref.current?.getMarkdown() ?? current
    await onSave(md)
    setDirty(false)
    void refetch()
  }

  const fetchRevision = async (id: string): Promise<string | null> => {
    if (!history) return null
    const qs = new URLSearchParams({ ...history, rev: id }).toString()
    const r = await fetch(`/api/history?${qs}`)
    if (!r.ok) return null
    return ((await r.json()) as { content: string }).content
  }

  /** Stage a revision's content in the editor (unsaved). */
  const loadRevision = async (id: string, prefetched?: string) => {
    const content = prefetched ?? (await fetchRevision(id))
    if (content === null) return
    setDiffing(null)
    setCurrent(content)
    setSeed((s) => s + 1)
    setDirty(true)
  }

  /** Show what changed between a revision and the editor's current content. */
  const openDiff = async (rev: Revision) => {
    const content = await fetchRevision(rev.id)
    if (content === null) return
    const now = ref.current?.getMarkdown() ?? current
    setDiffing({ rev, content, diff: diffLines(content, now) })
  }

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-6xl">
      <div
        className="flex h-[76vh] flex-col gap-3"
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 's') {
            e.preventDefault()
            if (editable && dirty && !saving) void save()
          }
        }}
      >
        {subtitle && <p className="text-xs text-muted">{subtitle}</p>}
        <div className="flex min-h-0 flex-1 gap-3">
          <div className="flex min-w-0 flex-1 flex-col">
            {diffing ? (
              <>
                <div className="mb-2 flex items-center gap-2 text-xs text-muted">
                  <span>
                    Changes since {relativeTime(diffing.rev.createdAt)}
                    {diffing.rev.createdBy ? ` · ${diffing.rev.createdBy}` : ''} — additions are what the current text
                    gained, removals what it lost.
                  </span>
                  {editable && (
                    <Button variant="outline" size="sm" className="ml-auto shrink-0" onClick={() => void loadRevision(diffing.rev.id, diffing.content)}>
                      <RotateCcw size={13} /> Load into editor
                    </Button>
                  )}
                  <Button variant="ghost" size="sm" className={cn('shrink-0', !editable && 'ml-auto')} onClick={() => setDiffing(null)}>
                    <X size={13} /> Close diff
                  </Button>
                </div>
                <div className="min-h-0 flex-1">
                  <DiffView diff={diffing.diff} fallback={diffing.content} />
                </div>
              </>
            ) : (
              <div className="min-h-0 flex-1 overflow-y-auto">
                <RichEditor
                  key={seed}
                  ref={ref}
                  value={current}
                  editable={editable}
                  onSave={() => setDirty(true)}
                  placeholder={editable ? 'Write in plain language — formatting is saved as markdown.' : undefined}
                  className="h-full"
                  fill
                />
              </div>
            )}
          </div>
          {showHistory && history && (
            <div className="w-64 shrink-0 overflow-y-auto rounded-xl border border-line-subtle p-1">
              <div className="px-2 py-1.5 text-[11px] uppercase tracking-wide text-muted">History</div>
              {revisions.length === 0 ? (
                <div className="px-2 py-2 text-xs text-muted">No saved revisions yet.</div>
              ) : (
                revisions.map((rev, i) => (
                  <div
                    key={rev.id}
                    className={cn(
                      'group flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs',
                      diffing?.rev.id === rev.id && 'bg-card',
                      i !== 0 && 'cursor-pointer transition-colors hover:bg-card',
                    )}
                    onClick={i === 0 ? undefined : () => void openDiff(rev)}
                    title={i === 0 ? undefined : 'Show changes since this revision'}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-fg">{i === 0 ? 'Current' : relativeTime(rev.createdAt)}</div>
                      <div className="truncate text-[11px] text-muted">
                        {rev.createdBy ?? 'unknown'} · {rev.size.toLocaleString()} chars
                      </div>
                    </div>
                    {editable && i !== 0 && (
                      <button
                        type="button"
                        title="Load this revision into the editor"
                        onClick={(e) => {
                          e.stopPropagation()
                          void loadRevision(rev.id)
                        }}
                        className="shrink-0 text-muted opacity-0 transition-all group-hover:opacity-100 hover:text-accent"
                      >
                        <RotateCcw size={13} />
                      </button>
                    )}
                  </div>
                ))
              )}
            </div>
          )}
        </div>
        <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
          {history && (
            <Button variant="ghost" size="sm" onClick={() => setShowHistory((v) => !v)}>
              <History size={14} className="mr-1.5" /> {showHistory ? 'Hide history' : 'History'}
            </Button>
          )}
          {footerExtra}
          <span className="ml-auto text-[11px] text-muted">{editable && dirty ? 'Unsaved changes · ⌘S to save' : ''}</span>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Close
          </Button>
          {editable && (
            <Button size="sm" onClick={() => void save()} disabled={saving || !dirty}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
          )}
        </div>
      </div>
    </Modal>
  )
}
