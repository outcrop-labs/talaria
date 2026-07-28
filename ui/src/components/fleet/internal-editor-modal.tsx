import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { motion } from 'framer-motion'
import { Check, ChevronLeft, History, RotateCcw, Sparkles, Square, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { CloseButton } from '@/components/ui/close-button'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { SkeletonRows } from '@/components/ui/skeleton'
import { relativeTime } from '@/lib/fleet'
import { streamMuse, type MuseKind } from '@/lib/muse'
import { cn } from '@/lib/cn'

interface Revision {
  id: string
  createdBy: string | null
  createdAt: string
  size: number
  /** Set for agent-version-backed kinds (soul/config/personality). */
  note?: string | null
  version?: number
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
        <p className="mb-2 text-xs text-muted">Too large to diff. Showing the revision's full content.</p>
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
  mode = 'rich',
  muse,
  nested = false,
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
  /** 'rich' (default) renders WYSIWYG markdown; 'plain' a mono text surface —
   *  for structured text like config YAML where prose rendering would lie. */
  mode?: 'rich' | 'plain'
  /** Render as a slide-in panel over the CURRENT dialog (the ticket-editor
   *  pattern) instead of stacking a second modal. Requires a positioned
   *  ancestor — the takeover Modal's panel provides one. */
  nested?: boolean
  /** Enable AI drafting for this document: prompt → streamed proposal →
   *  review diff → accept, iterating with the conversation retained. */
  muse?: { kind: MuseKind; context?: string }
}) {
  const ref = useRef<RichEditorHandle>(null)
  const [dirty, setDirty] = useState(false)
  const [seed, setSeed] = useState(0) // bump to remount the editor with new content
  const [current, setCurrent] = useState(value)
  const [showHistory, setShowHistory] = useState(true)
  const [diffing, setDiffing] = useState<{ rev: Revision; content: string; diff: DiffLine[] | null } | null>(null)

  // ── Muse: prompt → streamed proposal → review/accept, iteratively ───────
  const [museOpen, setMuseOpen] = useState(false)
  const [instruction, setInstruction] = useState('')
  const [generating, setGenerating] = useState(false)
  const [proposal, setProposal] = useState<string | null>(null)
  const [proposalDiff, setProposalDiff] = useState<DiffLine[] | null | 'text'>('text')
  const [chat, setChat] = useState<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  const [museError, setMuseError] = useState<string | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const streamRef = useRef<HTMLPreElement | null>(null)

  useEffect(() => {
    if (generating && streamRef.current) streamRef.current.scrollTop = streamRef.current.scrollHeight
  }, [proposal, generating])
  useEffect(() => () => abortRef.current?.abort(), [])

  const editorText = () => ref.current?.getMarkdown() ?? current

  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || !muse) return
    setGenerating(true)
    setMuseError(null)
    setDiffing(null)
    setProposal('')
    setProposalDiff('text')
    const ac = new AbortController()
    abortRef.current = ac
    try {
      const full = await streamMuse(
        { kind: muse.kind, context: muse.context, current: editorText(), instruction: instr, chat },
        (piece) => setProposal((p) => (p ?? '') + piece),
        ac.signal,
      )
      setChat((c) => [...c.slice(-10), { role: 'user', content: instr }, { role: 'assistant', content: full }])
      setInstruction('')
    } catch (e) {
      if (!ac.signal.aborted) {
        setMuseError((e as Error).message)
        setProposal(null)
      }
    } finally {
      setGenerating(false)
    }
  }

  const acceptProposal = () => {
    if (proposal === null) return
    setCurrent(proposal)
    setSeed((s) => s + 1)
    setDirty(true)
    setProposal(null)
  }

  const { data: revisions = [], isLoading: revisionsLoading, refetch } = useQuery({
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

  // Nested mode: Esc closes THIS panel only — a capture-phase listener runs
  // before (and suppresses) the parent modal's document-level Esc handler.
  useEffect(() => {
    if (!nested || !open) return
    const h = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('keydown', h, true)
    return () => document.removeEventListener('keydown', h, true)
  }, [nested, open, onClose])

  const body = (
      <div
        className={cn('flex flex-col gap-3', nested ? 'h-full min-h-0' : 'h-[76vh]')}
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
            {proposal !== null ? (
              <>
                <div className="mb-2 flex items-center gap-2 text-xs text-muted">
                  <Sparkles size={13} className="shrink-0 text-accent" />
                  {generating ? (
                    <>
                      <span>Drafting</span>
                      <Button variant="ghost" size="sm" className="ml-auto shrink-0" onClick={() => abortRef.current?.abort()}>
                        <Square size={12} fill="currentColor" /> Stop
                      </Button>
                    </>
                  ) : (
                    <>
                      <span>Proposal: review, then accept or refine below.</span>
                      <Button size="sm" className="ml-auto shrink-0" onClick={acceptProposal}>
                        <Check size={13} /> Accept
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        className="shrink-0"
                        onClick={() => setProposalDiff((d) => (d === 'text' ? diffLines(editorText(), proposal) : 'text'))}
                      >
                        {proposalDiff === 'text' ? 'View changes' : 'View text'}
                      </Button>
                      <Button variant="ghost" size="sm" className="shrink-0" onClick={() => setProposal(null)}>
                        <X size={13} /> Discard
                      </Button>
                    </>
                  )}
                </div>
                <div className="min-h-0 flex-1">
                  {proposalDiff === 'text' || generating ? (
                    <pre
                      ref={streamRef}
                      className="h-full overflow-y-auto whitespace-pre-wrap rounded-xl border border-[var(--theme-accent-border,var(--theme-accent))] p-3 font-[var(--font-mono)] text-xs leading-5 text-fg"
                    >
                      {proposal}
                      {generating && <span className="animate-pulse text-accent">▍</span>}
                    </pre>
                  ) : (
                    <DiffView diff={proposalDiff} fallback={proposal} />
                  )}
                </div>
              </>
            ) : diffing ? (
              <>
                <div className="mb-2 flex items-center gap-2 text-xs text-muted">
                  <span>
                    Changes since {diffing.rev.version !== undefined ? `v${diffing.rev.version} · ` : ''}
                    {relativeTime(diffing.rev.createdAt)}
                    {diffing.rev.createdBy ? ` · ${diffing.rev.createdBy}` : ''}. Additions are what the current text
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
            ) : mode === 'plain' ? (
              editable ? (
                <textarea
                  key={seed}
                  value={current}
                  onChange={(e) => {
                    setCurrent(e.target.value)
                    setDirty(true)
                  }}
                  spellCheck={false}
                  className="min-h-0 w-full flex-1 resize-none rounded-xl border border-line bg-[var(--theme-input)] p-3 font-[var(--font-mono)] text-xs leading-5 text-fg outline-none focus:border-accent"
                />
              ) : (
                <pre className="min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap rounded-xl border border-line-subtle p-3 font-[var(--font-mono)] text-xs leading-5 text-fg">
                  {current}
                </pre>
              )
            ) : (
              <div className="min-h-0 flex-1 overflow-y-auto">
                <RichEditor
                  key={seed}
                  ref={ref}
                  value={current}
                  editable={editable}
                  onSave={() => setDirty(true)}
                  placeholder={editable ? 'Write in plain language. Formatting is saved as markdown.' : undefined}
                  className="h-full"
                  fill
                />
              </div>
            )}
          </div>
          {showHistory && history && (
            <div className="w-64 shrink-0 overflow-y-auto rounded-xl border border-line-subtle p-1">
              <div className="px-2 py-1.5 text-[11px] uppercase tracking-wide text-muted">History</div>
              {revisionsLoading ? (
                <SkeletonRows rows={4} className="px-2 py-2" />
              ) : revisions.length === 0 ? (
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
                      <div className="truncate text-fg">
                        {rev.version !== undefined && <span className="mr-1.5 font-[var(--font-mono)] text-accent">v{rev.version}</span>}
                        {i === 0 ? 'Current' : relativeTime(rev.createdAt)}
                      </div>
                      <div className="truncate text-[11px] text-muted">
                        {rev.createdBy ?? 'unknown'} · {rev.size.toLocaleString()} chars
                      </div>
                      {rev.note && <div className="truncate text-[11px] italic text-muted">{rev.note}</div>}
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
        {museOpen && muse && editable && (
          <div className="flex items-end gap-2.5">
            <Sparkles size={14} className="mb-3 shrink-0 text-accent" />
            <Textarea
              autoGrow
              rows={1}
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  if (!generating && instruction.trim()) void generate()
                }
              }}
              placeholder={
                proposal !== null
                  ? 'Refine the proposal, e.g. “shorter, and add a step for weekends”'
                  : 'Describe what you want. It drafts from the current version'
              }
              className="max-h-40 text-sm"
              autoFocus
            />
            <Button className="shrink-0" onClick={() => void generate()} disabled={generating || !instruction.trim()}>
              {generating ? 'Drafting' : proposal !== null ? 'Refine' : 'Draft'}
            </Button>
          </div>
        )}
        {museError && <p className="text-xs text-[color:var(--theme-danger)]">{museError}</p>}
        <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
          {muse && editable && (
            <Button
              variant={museOpen ? 'outline' : 'ghost'}
              size="sm"
              onClick={() => setMuseOpen((v) => !v)}
              title="Draft with AI. Uses your preferred model (Settings)"
            >
              <Sparkles size={14} className="mr-1.5" /> Muse
            </Button>
          )}
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
              {saving ? 'Saving' : 'Save'}
            </Button>
          )}
        </div>
      </div>
  )

  if (nested) {
    if (!open) return null
    return (
      <motion.div
        initial={{ x: '100%' }}
        animate={{ x: 0 }}
        transition={{ type: 'tween', duration: 0.18, ease: 'easeOut' }}
        className="absolute inset-0 z-30 flex flex-col bg-[var(--theme-panel)]"
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-line-subtle px-6 py-3.5">
          <button
            type="button"
            onClick={onClose}
            title="Back"
            className="grid h-7 w-7 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg"
          >
            <ChevronLeft size={16} />
          </button>
          <div className="text-sm font-semibold text-fg">{title}</div>
          <CloseButton onClick={onClose} className="ml-auto" />
        </div>
        <div className="min-h-0 flex-1 p-6">{body}</div>
      </motion.div>
    )
  }

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-6xl">
      {body}
    </Modal>
  )
}
