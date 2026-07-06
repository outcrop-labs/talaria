import { useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { History, RotateCcw } from 'lucide-react'
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

/** A premium editor modal for an agent-internal document (skill / memory):
 *  WYSIWYG editing on the left, version history on the right. Reverting loads
 *  the older content into the editor (not saved until you Save), so a revert is
 *  itself reviewable and produces a new revision. */
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
  const [showHistory, setShowHistory] = useState(false)

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

  const loadRevision = async (id: string) => {
    if (!history) return
    const qs = new URLSearchParams({ ...history, rev: id }).toString()
    const r = await fetch(`/api/history?${qs}`)
    if (!r.ok) return
    const { content } = (await r.json()) as { content: string }
    setCurrent(content)
    setSeed((s) => s + 1)
    setDirty(true)
  }

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-4xl">
      <div className="space-y-3">
        {subtitle && <p className="text-xs text-muted">{subtitle}</p>}
        <div className="flex min-h-[26rem] gap-3">
          <div className="min-w-0 flex-1">
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
          {showHistory && history && (
            <div className="w-56 shrink-0 overflow-y-auto rounded-xl border border-line-subtle p-1">
              <div className="px-2 py-1.5 text-[11px] uppercase tracking-wide text-muted">History</div>
              {revisions.length === 0 ? (
                <div className="px-2 py-2 text-xs text-muted">No saved revisions yet.</div>
              ) : (
                revisions.map((rev, i) => (
                  <div key={rev.id} className={cn('flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs', i === 0 && 'text-fg')}>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-fg">{i === 0 ? 'Current' : relativeTime(rev.createdAt)}</div>
                      <div className="truncate text-[11px] text-muted">
                        {rev.createdBy ?? 'unknown'} · {rev.size} chars
                      </div>
                    </div>
                    {editable && i !== 0 && (
                      <button
                        type="button"
                        title="Load this revision into the editor"
                        onClick={() => void loadRevision(rev.id)}
                        className="shrink-0 text-muted transition-colors hover:text-accent"
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
          <span className="ml-auto" />
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
