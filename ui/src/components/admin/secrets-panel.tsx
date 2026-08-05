// Admin → Secrets. Everything this instance holds sealed, in one list, with
// per-row health and a per-row way out.
//
// The shape follows from two facts. Values can never be shown, so the row's
// job is provenance and health rather than content — what it is, what breaks
// without it, when it was set, whether it still decrypts. And an operator
// arrives here in one of two moods: "what does this instance have?" (browsing)
// or "three things broke at once" (recovering). The unreadable strip at the
// top serves the second without making the first read like an incident.

import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { Panel } from '@/components/ui/panel'
import { SectionHeader } from '@/components/ui/section-header'
import { Chip, DangerLink, StatusDot } from '@/components/ui/chip'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { QueryError } from '@/components/ui/query-state'
import { Skeleton } from '@/components/ui/skeleton'
import { relativeTime } from '@/lib/fleet'
import { cn } from '@/lib/cn'
import {
  GROUP_LABELS,
  STATE_COPY,
  useClearSecret,
  useSecretHealth,
  type RootHealth,
  type SecretGroup,
  type SecretRow,
} from '@/lib/secrets'

const GROUP_ORDER: SecretGroup[] = ['models', 'integrations', 'agents', 'platform']

const STATE_TONE = {
  ok: 'success',
  unreadable: 'danger',
  missing: 'neutral',
  env: 'neutral',
} as const

function StateChip({ row }: { row: SecretRow }) {
  const copy = STATE_COPY[row.state]
  return (
    <Chip tone={STATE_TONE[row.state]} title={copy.hint}>
      {copy.label}
    </Chip>
  )
}

// ── The root ─────────────────────────────────────────────────────────────────
// Deliberately the first thing on the page, and deliberately not editable. It
// lives in the process environment, so the app can only report it — and saying
// it exists at all is most of the value, because nothing else in the product
// mentions it until the day it stops matching.
function RootCard({ root }: { root: RootHealth }) {
  const tone =
    root.state === 'ok' ? 'ok' : root.state === 'fallback' ? 'warn' : ('danger' as const)
  const headline =
    root.state === 'ok'
      ? `Set from ${root.name}`
      : root.state === 'fallback'
        ? 'Borrowing AUTH_SECRET'
        : root.state === 'absent'
          ? 'Not set'
          : 'Does not match this database'
  const body =
    root.state === 'ok'
      ? 'Every secret below is sealed with a data key wrapped by this value. Back it up with the database — a dump restored without it restores an instance that cannot read its own secrets.'
      : root.state === 'fallback'
        ? 'TALARIA_SECRET_KEY is not set, so AUTH_SECRET is doing this job. Rotating AUTH_SECRET would make every secret below unrecoverable. Set TALARIA_SECRET_KEY to the current AUTH_SECRET value to pin it.'
        : root.state === 'absent'
          ? 'Neither TALARIA_SECRET_KEY nor AUTH_SECRET is set, so nothing new can be sealed. Set TALARIA_SECRET_KEY and restart.'
          : (root.failure ??
            'The root secret this process has is not the one these secrets were sealed with.')

  return (
    <div
      className={cn(
        'rounded-md border p-4',
        root.state === 'ok' ? 'border-line' : root.state === 'fallback' ? 'border-warning/40' : 'border-danger/40',
      )}
    >
      <div className="mb-1.5 flex flex-wrap items-center gap-2">
        <StatusDot status={tone} />
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Encryption root</span>
        <span className="text-sm font-medium text-fg">{headline}</span>
        <span className="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
          {root.activeVersion ? `key v${root.activeVersion}` : 'no key'}
          {root.storedVersions > root.loadedVersions.length &&
            ` · ${root.storedVersions - root.loadedVersions.length} version${root.storedVersions - root.loadedVersions.length === 1 ? '' : 's'} unreadable`}
        </span>
      </div>
      <p className="text-xs leading-relaxed text-muted">{body}</p>
    </div>
  )
}

// ── One row ──────────────────────────────────────────────────────────────────
function Row({ row, onClear, busy }: { row: SecretRow; onClear: (row: SecretRow) => void; busy: boolean }) {
  return (
    <li className="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="truncate text-sm text-fg">{row.label}</span>
          {row.owner && <span className="truncate font-mono text-[10px] text-ink-dim">{row.owner}</span>}
          <StateChip row={row} />
        </div>
        {/* What breaks without it. The reason an operator can decide anything
            here without going and reading the code. */}
        <p className="mt-0.5 text-xs leading-relaxed text-muted">{row.unlocks}</p>
      </div>
      <div className="flex shrink-0 items-center gap-3 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
        {row.setAt && <span title={new Date(row.setAt).toLocaleString()}>set {relativeTime(row.setAt)}</span>}
        {row.expiresAt && <span title={new Date(row.expiresAt).toLocaleString()}>expires {relativeTime(row.expiresAt)}</span>}
        {row.lastUsedAt && <span title={new Date(row.lastUsedAt).toLocaleString()}>used {relativeTime(row.lastUsedAt)}</span>}
        {row.href ? (
          <Link to={row.href} className="underline-offset-2 transition-colors hover:text-fg hover:underline">
            {row.state === 'missing' ? 'Set up' : 'Replace'}
          </Link>
        ) : (
          // No deep link because the value belongs to a person: it is replaced
          // in their own Settings, by them. Say so rather than offering a link
          // that would land an admin on their own page.
          <span title={`Replaced by ${row.owner ?? 'the owner'} in ${row.surface}`}>{row.surface}</span>
        )}
        {row.clearable && (
          <DangerLink onClick={() => onClear(row)} disabled={busy}>
            Clear
          </DangerLink>
        )}
      </div>
    </li>
  )
}

export function SecretsPanel() {
  const query = useSecretHealth()
  const { data, isPending } = query
  const clear = useClearSecret()
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)

  const clearOne = async (row: SecretRow) => {
    const ok = await confirm({
      title: `Clear ${row.label}?`,
      // Every clear names the consequence and the way back. This is the same
      // contract scripts/reset.sh keeps, at the granularity of one row.
      message: `This deletes the stored value and nothing else. What stops working: ${row.unlocks}. To restore it, enter the value again in ${row.surface}.${
        row.id.startsWith('agent-key:')
          ? ' The agent will need a fleet re-render to get a new credential.'
          : ''
      }`,
      confirmLabel: 'Clear',
    })
    if (!ok) return
    setBusy(true)
    setMsg(null)
    try {
      const res = await clear({ id: row.id })
      setMsg(res.error ?? (res.changed ? `Cleared ${row.label}.` : `${row.label} was already cleared.`))
    } finally {
      setBusy(false)
    }
  }

  const clearAllUnreadable = async () => {
    const rows = (data?.rows ?? []).filter((r) => r.state === 'unreadable' && r.clearable)
    const ok = await confirm({
      title: `Clear ${rows.length} unreadable secret${rows.length === 1 ? '' : 's'}?`,
      // Naming them is the whole point: "clear all" without a list is how an
      // operator ends up destroying something they could still have recovered.
      message: `These will be deleted:\n\n${rows.map((r) => `· ${r.label}${r.owner ? ` (${r.owner})` : ''} — re-enter in ${r.surface}`).join('\n')}\n\nEverything readable is left alone. If you still have the original root secret anywhere, restoring it recovers these instead.`,
      confirmLabel: 'Clear them',
    })
    if (!ok) return
    setBusy(true)
    setMsg(null)
    try {
      const res = await clear({ unreadable: true })
      setMsg(
        res.error ??
          `Cleared ${res.cleared?.length ?? 0}${res.failed?.length ? ` · ${res.failed.length} could not be cleared (see server logs)` : ''}.`,
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <SectionHeader
        className="mb-4"
        title="Secrets"
        info="Every credential this instance holds, wherever it was entered. Values are never shown — sealed secrets cannot be read back, only replaced. Each row says what it unlocks and whether this instance can still decrypt it."
      />

      {isPending ? (
        <div className="space-y-3">
          <Skeleton className="h-16 w-full rounded-md" />
          {Array.from({ length: 5 }, (_, i) => (
            <div key={i} className="space-y-1.5 py-1">
              <Skeleton className="h-3 w-48 rounded-full" delay={i * 0.1} />
              <Skeleton className="h-2.5 w-72 rounded-full" delay={i * 0.1 + 0.05} />
            </div>
          ))}
        </div>
      ) : !data ? (
        // An empty inventory over a failed read would read as "this instance
        // holds nothing" — the most reassuring possible way to be wrong.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load the secrets inventory"
          onRetry={() => void query.refetch()}
        />
      ) : (
        <div className="space-y-5">
          <RootCard root={data.root} />

          {data.counts.unreadable > 0 && (
            <div className="rounded-md border border-danger/40 p-4">
              <div className="mb-2 flex items-center gap-1.5">
                <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">
                  {data.counts.unreadable} unreadable
                </span>
                <span className="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
              </div>
              <p className="mb-3 text-xs leading-relaxed text-muted">
                These were sealed with a key this instance no longer has. If you still have the original root secret
                anywhere, restore it — that recovers them. Clearing deletes them so the value can be entered again;
                everything readable is left alone.
              </p>
              <Button size="sm" variant="danger" onClick={() => void clearAllUnreadable()} disabled={busy}>
                Clear all unreadable
              </Button>
            </div>
          )}

          {msg && <p className="text-xs text-muted">{msg}</p>}

          {GROUP_ORDER.map((group) => {
            const rows = data.rows.filter((r) => r.group === group)
            if (!rows.length) return null
            return (
              <section key={group}>
                <h3 className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                  {GROUP_LABELS[group]}
                </h3>
                <ul className="divide-y divide-line-subtle">
                  {rows.map((r) => (
                    <Row key={r.id} row={r} onClear={(row) => void clearOne(row)} busy={busy} />
                  ))}
                </ul>
              </section>
            )
          })}

          {!data.rows.length && (
            <p className="text-xs text-muted">
              Nothing configured yet. Secrets appear here as you add them — a provider key on Models, an integration,
              an agent secret.
            </p>
          )}
        </div>
      )}
    </Panel>
  )
}
