import { createPortal } from 'react-dom'
import { useEffect, useState, type ReactNode } from 'react'
import {
  CalendarDays,
  ChevronRight,
  Clock3,
  ExternalLink,
  Inbox,
  Mail,
  X,
} from 'lucide-react'
import { INBOX_SNOOZE_OPTIONS, useInboxFocusWorkspace } from '@/components/inbox/inbox-focus-shell'
import { Button, buttonClasses } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import {
  type FocusAction,
  type FocusItem,
} from '@/lib/inbox-focus'

const PIPELINE = ['Signal', 'Triage', 'Execute', 'Validate', 'Close'] as const
function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  return target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
}

function stageFor(item: FocusItem): number {
  if (item.sourceType === 'approval') return 2
  if (item.statusLabel.includes('REVIEW')) return 3
  if (item.statusLabel.includes('BLOCKED') || item.statusLabel.includes('TRIAGE')) return 1
  if (item.sourceType === 'channel') return 1
  return 0
}

function metadataValue(value: string | number | boolean | null): string {
  if (value === null) return '—'
  if (typeof value === 'boolean') return value ? 'Yes' : 'No'
  return String(value).replaceAll('_', ' ')
}

function priorityClass(priority: FocusItem['priority']): string {
  if (priority === 'p0') return 'text-accent'
  if (priority === 'p1') return 'text-danger'
  if (priority === 'p2') return 'text-muted'
  return 'text-success'
}

function sourceLabel(item: FocusItem): string {
  if (item.sourceType === 'channel') return 'Comms'
  return item.sourceType[0]!.toUpperCase() + item.sourceType.slice(1)
}

export function FocusInbox({ mail, agenda }: { mail: ReactNode; agenda: ReactNode }) {
  const [drawer, setDrawer] = useState<'mail' | 'agenda' | null>(null)
  const {
    data,
    isLoading,
    isError,
    refetch,
    orderedItems,
    active,
    recommendedAction,
    busyAction,
    snoozeMs,
    setSnoozeMs,
    performAction,
    snooze,
    skip,
  } = useInboxFocusWorkspace()

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!active || busyAction !== null || isEditable(event.target) || event.metaKey || event.ctrlKey || event.altKey) return
      const key = event.key.toLowerCase()
      if (key === 'a') {
        event.preventDefault()
        if (recommendedAction) void performAction(active, recommendedAction.id)
        else window.location.assign(active.sourceHref)
      }
      if (key === 'o') {
        event.preventDefault()
        window.location.assign(active.sourceHref)
      }
      if (key === 's') {
        event.preventDefault()
        void snooze()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [active, busyAction, performAction, recommendedAction, snooze])

  return (
    <>
      <div className="h-full min-h-0 min-w-0 overflow-y-auto px-4 pb-8 pt-8 sm:px-8 sm:pt-12">
        <main className="mx-auto w-full max-w-[760px]">
          <FocusHeader
            count={data?.counts.total ?? 0}
            current={active ? 1 : 0}
            onOpenMail={() => setDrawer('mail')}
            onOpenAgenda={() => setDrawer('agenda')}
          />

          {isLoading ? (
            <FocusLoading />
          ) : isError ? (
            <FocusOffline onRetry={() => void refetch()} />
          ) : active ? (
            <>
              <FocusCard
                item={active}
                recommendedAction={recommendedAction}
                busyAction={busyAction}
                snoozeMs={snoozeMs}
                onSnoozeMs={setSnoozeMs}
                onAction={(action) => void performAction(active, action.id)}
                onSnooze={() => void snooze()}
                onSkip={skip}
                canSkip={orderedItems.length > 1}
              />
              <QueuePreview items={orderedItems.slice(1, 5)} remaining={Math.max(0, orderedItems.length - 1)} />
            </>
          ) : (
            <InboxZero />
          )}
        </main>
      </div>

      {drawer && (
        <UtilityDrawer title={drawer === 'mail' ? 'Mail' : 'Agenda'} onClose={() => setDrawer(null)}>
          {drawer === 'mail' ? mail : agenda}
        </UtilityDrawer>
      )}
    </>
  )
}

function FocusHeader({
  count,
  current,
  onOpenMail,
  onOpenAgenda,
}: {
  count: number
  current: number
  onOpenMail: () => void
  onOpenAgenda: () => void
}) {
  return (
    <header className="flex flex-wrap items-center gap-2 border-b border-line pb-4 sm:gap-3">
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        <h1 className="font-sans text-lg font-medium tracking-tight text-fg">Inbox</h1>
        <span className="font-mono text-[11px] tabular-nums tracking-[0.06em] text-muted">· {count}</span>
      </div>
      <span className="hidden font-mono text-[10px] tabular-nums tracking-[0.08em] text-ink-dim sm:block">
        {String(current).padStart(2, '0')} / {String(count).padStart(2, '0')}
      </span>
      <button type="button" onClick={onOpenMail} className="flex h-8 items-center gap-1.5 rounded-md border border-line px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:bg-hover hover:text-fg" aria-label="Open Mail drawer">
        <Mail size={12} /> Mail
      </button>
      <button type="button" onClick={onOpenAgenda} className="flex h-8 items-center gap-1.5 rounded-md border border-line px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:bg-hover hover:text-fg" aria-label="Open Agenda drawer">
        <CalendarDays size={12} /> Agenda
      </button>
    </header>
  )
}

function FocusCard({
  item,
  recommendedAction,
  busyAction,
  snoozeMs,
  onSnoozeMs,
  onAction,
  onSnooze,
  onSkip,
  canSkip,
}: {
  item: FocusItem
  recommendedAction: FocusAction | null
  busyAction: string | null
  snoozeMs: number
  onSnoozeMs: (value: number) => void
  onAction: (action: FocusAction) => void
  onSnooze: () => void
  onSkip: () => void
  canSkip: boolean
}) {
  const stage = stageFor(item)
  const otherActions = item.actions.filter((action) => action.id !== recommendedAction?.id)
  const meta = Object.entries(item.metadata).filter(([, value]) => value !== null).slice(0, 5)
  return (
    <section aria-labelledby="focus-question" className="border-b border-line py-10 sm:py-12">
      <div className="mb-3 flex flex-wrap items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
        <span>Next up</span><span>·</span>
        <span className={priorityClass(item.priority)}>{item.priority}</span><span>·</span>
        <span>{item.statusLabel}</span><span>·</span>
        <span>{relativeTime(item.createdAt)}</span>
        {item.briefStatus === 'pending' && <span className="ml-auto text-ink-dim">Scout is refining</span>}
      </div>
      <h2 id="focus-question" className="max-w-[720px] font-sans text-[32px] font-light leading-[1.15] tracking-[-0.025em] text-fg sm:text-[40px] sm:leading-[48px]">
        {item.question}
      </h2>
      <p className="mt-3 max-w-[680px] font-sans text-sm leading-5 text-muted">{item.recommendation}</p>

      <div className="mt-5 flex flex-wrap gap-x-5 gap-y-1 font-mono text-[10px] uppercase tracking-[0.055em] text-ink-dim">
        <span>{sourceLabel(item)}</span>
        {meta.map(([key, value]) => <span key={key}>{key.replaceAll('_', ' ')}: <span className="text-muted">{metadataValue(value)}</span></span>)}
      </div>

      {item.evidence.length > 0 && (
        <div className="mt-5 grid gap-px overflow-hidden rounded-md border border-line bg-line sm:grid-cols-2">
          {item.evidence.slice(0, 4).map((evidence) => (
            <div key={`${evidence.label}:${evidence.text}`} className="bg-panel px-3 py-3">
              <div className="mb-1 font-mono text-[9px] uppercase tracking-[0.08em] text-ink-dim">{evidence.label}</div>
              <p className="line-clamp-3 font-sans text-xs leading-[18px] text-muted">{evidence.text}</p>
            </div>
          ))}
        </div>
      )}

      <ol aria-label="Decision progress" className="mt-6 grid grid-cols-5 gap-1.5">
        {PIPELINE.map((label, index) => (
          <li key={label} className={cn('flex h-[30px] min-w-0 items-center justify-center rounded border font-mono text-[9px] uppercase tracking-[0.04em]', index === stage ? 'border-success bg-success/10 text-success' : index < stage ? 'border-line-strong text-muted' : 'border-line text-ink-dim')}>
            <span className="hidden sm:inline">0{index + 1} · </span>{label}
          </li>
        ))}
      </ol>

      <div className="mt-6 flex flex-wrap items-center gap-2">
        {recommendedAction ? (
          <Button size="sm" onClick={() => onAction(recommendedAction)} disabled={busyAction !== null}>
            {busyAction === recommendedAction.id ? 'Working' : recommendedAction.label}
          </Button>
        ) : (
          <a href={item.sourceHref} className={buttonClasses({ size: 'sm' })}>Open source</a>
        )}
        {otherActions.map((action) => (
          <Button key={action.id} size="sm" variant="outline" onClick={() => onAction(action)} disabled={busyAction !== null}>
            {action.label}
          </Button>
        ))}
        {recommendedAction && (
          <a href={item.sourceHref} className={buttonClasses({ variant: 'outline', size: 'sm' })}>
            <ExternalLink size={12} /> View source
          </a>
        )}
        <div className="flex items-center rounded-md border border-line bg-raised">
          <label className="sr-only" htmlFor="focus-snooze">Snooze duration</label>
          <select id="focus-snooze" value={snoozeMs} onChange={(event) => onSnoozeMs(Number(event.target.value))} className="h-8 bg-transparent pl-2 font-mono text-[10px] uppercase tracking-[0.04em] text-muted outline-none">
            {INBOX_SNOOZE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select>
          <button type="button" onClick={onSnooze} disabled={busyAction !== null} className="grid h-8 w-8 place-items-center text-muted hover:text-fg disabled:opacity-50" aria-label="Snooze item">
            <Clock3 size={12} />
          </button>
        </div>
        <button type="button" onClick={onSkip} disabled={!canSkip || busyAction !== null} className="ml-auto flex h-8 items-center gap-1.5 px-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted hover:text-fg disabled:opacity-40">
          Skip <ChevronRight size={12} />
        </button>
      </div>
      <div className="mt-2 flex gap-4 font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim">
        <span>A · Primary</span><span>O · Open</span><span>S · Snooze</span>
      </div>
    </section>
  )
}

function QueuePreview({ items, remaining }: { items: FocusItem[]; remaining: number }) {
  return (
    <section aria-labelledby="queue-heading" className="py-8">
      <div className="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        <h2 id="queue-heading">In queue</h2><span>· {remaining}</span>
      </div>
      {items.length === 0 ? (
        <p className="font-sans text-sm text-muted">No other decisions are waiting.</p>
      ) : (
        <ol className="divide-y divide-line border-y border-line">
          {items.map((item) => (
            <li key={item.key} className="grid min-h-11 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 py-2">
              <span className={cn('font-mono text-[10px] uppercase tracking-[0.06em]', priorityClass(item.priority))}>{item.priority}</span>
              <span className="min-w-0 truncate font-sans text-[13px] text-muted">{item.question}</span>
              <span className="font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{relativeTime(item.createdAt)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  )
}

function InboxZero() {
  return (
    <section className="grid min-h-[430px] place-items-center border-b border-line text-center">
      <div>
        <span className="mx-auto mb-4 grid h-12 w-12 place-items-center rounded-full border border-success/50 text-success"><Inbox size={20} /></span>
        <h2 className="font-sans text-[32px] font-light tracking-[-0.02em] text-fg">Inbox zero</h2>
        <p className="mt-2 font-sans text-sm text-muted">No decisions are waiting. Scout is still available below.</p>
      </div>
    </section>
  )
}

function FocusLoading() {
  return (
    <div className="space-y-4 py-12">
      <Skeleton className="h-3 w-44 rounded-full" />
      <Skeleton className="h-12 w-4/5 rounded-md" />
      <Skeleton className="h-4 w-3/5 rounded-full" />
      <div className="grid grid-cols-5 gap-2 pt-5">{PIPELINE.map((label) => <Skeleton key={label} className="h-[30px] rounded" />)}</div>
      <Skeleton className="mt-6 h-9 w-56 rounded-md" />
    </div>
  )
}

function FocusOffline({ onRetry }: { onRetry: () => void }) {
  return (
    <section className="grid min-h-[430px] place-items-center text-center">
      <div>
        <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Queue offline</div>
        <h2 className="mt-2 font-sans text-2xl font-light text-fg">The source-first queue could not load.</h2>
        <p className="mx-auto mt-2 max-w-md font-sans text-sm text-muted">No action was taken. Retry when the local service is available.</p>
        <Button className="mt-5" size="sm" variant="outline" onClick={onRetry}>Retry</Button>
      </div>
    </section>
  )
}

function UtilityDrawer({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === 'Escape' && onClose()
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])
  if (typeof document === 'undefined') return null
  return createPortal(
    <div className="fixed inset-0 z-50" role="dialog" aria-modal aria-label={`${title} drawer`}>
      <button type="button" aria-label="Close drawer" onClick={onClose} className="absolute inset-0 bg-black/50" />
      <aside className="gd-enter absolute inset-y-0 right-0 flex w-full max-w-[560px] flex-col border-l border-line bg-surface shadow-[var(--theme-shadow-3)]">
        <header className="flex h-14 shrink-0 items-center border-b border-line px-5">
          <h2 className="font-mono text-[11px] uppercase tracking-[0.08em] text-fg">{title}</h2>
          <button type="button" onClick={onClose} className="ml-auto grid h-8 w-8 place-items-center rounded-md text-muted hover:bg-hover hover:text-fg" aria-label={`Close ${title}`}><X size={15} /></button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
      </aside>
    </div>,
    document.body,
  )
}
