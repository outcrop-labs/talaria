import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { submitOnEnter } from '@/components/ui/control'
import { EmptyState } from '@/components/ui/empty-state'
import { QueryState } from '@/components/ui/query-state'
import { Skeleton } from '@/components/ui/skeleton'
import { getList } from '@/lib/fetch-json'
import { relativeTime } from '@/lib/fleet'

interface SecretMeta {
  name: string
  updatedBy: string | null
  updatedAt: string
}

// Per-agent secrets: UI-configured env vars, encrypted at rest, write-only
// (values never come back). Materialized into the agent's env at render — no
// hand edits to fleet/.env unless you're an advanced deployer who wants to.
export function SecretsTab({ agentId }: { agentId: string }) {
  const qc = useQueryClient()
  const key = ['agent-secrets', agentId]
  // GET is 200 `{ secrets }` or 403 — there is no 404 here, so EVERY non-2xx is
  // a failure. It used to answer `[]`, which rendered "No secrets": an operator
  // reading that would go add the token that is already there.
  const query = useQuery({
    queryKey: key,
    queryFn: (): Promise<SecretMeta[]> => getList<SecretMeta>(`/api/fleet/agents/${agentId}/secrets`, 'secrets'),
  })
  const [name, setName] = useState('')
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const nameOk = /^[A-Z][A-Z0-9_]{1,63}$/.test(name)

  const save = async () => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch(`/api/fleet/agents/${agentId}/secrets`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name, value }),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) return setErr(j.error ?? 'could not save')
      setName('')
      setValue('')
      await qc.invalidateQueries({ queryKey: key })
    } finally {
      setBusy(false)
    }
  }

  const remove = async (n: string) => {
    if (!(await confirm({ title: 'Remove secret', message: `Remove ${n}? The agent loses it on its next start.`, confirmLabel: 'Remove', danger: true }))) return
    await fetch(`/api/fleet/agents/${agentId}/secrets?name=${encodeURIComponent(n)}`, { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: key })
  }

  return (
    <div className="space-y-4">
      <p className="font-sans text-xs leading-relaxed text-muted">
        Environment variables just for this agent: a vendor token, a service key. Encrypted at rest, write-only here,
        and injected into the container when it starts from Talaria (Start recreates it with the latest values).
      </p>
      <QueryState
        query={query}
        errorTitle="Could not load this agent's secrets"
        errorVariant="compact"
        skeleton={
          <ul aria-hidden className="divide-y divide-line rounded-lg border border-line">
            {[0, 1].map((i) => (
              <li key={i} className="flex items-center gap-3 px-3.5 py-3.5">
                <Skeleton className="h-3 w-32 rounded-full" delay={i * 0.12} />
                <Skeleton className="h-2.5 w-14 rounded-full" delay={i * 0.12 + 0.12} />
                <Skeleton className="ml-auto h-2.5 w-24 rounded-full" delay={i * 0.12 + 0.24} />
              </li>
            ))}
          </ul>
        }
        empty={<EmptyState icon={<KeyRound size={22} />} title="No secrets" hint="Everything it needs comes from the shared platform env." />}
      >
        {(secrets) => (
          <ul className="divide-y divide-line rounded-lg border border-line">
            {secrets.map((s) => (
              <li key={s.name} className="flex items-center gap-3 px-3.5 py-2.5 transition-colors hover:bg-hover">
                <code className="font-mono text-sm text-fg">{s.name}</code>
                <span className="font-mono text-xs text-muted">••••••••</span>
                <span className="ml-auto shrink-0 font-mono text-[11px] text-muted">
                  {s.updatedBy ?? 'unknown'} · {relativeTime(s.updatedAt)}
                </span>
                <button
                  type="button"
                  title="Remove"
                  onClick={() => void remove(s.name)}
                  className="shrink-0 text-muted transition-colors hover:text-danger"
                >
                  <Trash2 size={14} />
                </button>
              </li>
            ))}
          </ul>
        )}
      </QueryState>
      <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto]">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
          onKeyDown={submitOnEnter(() => !busy && nameOk && value && void save())}
          placeholder="FIGMA_TOKEN"
        />
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={submitOnEnter(() => !busy && nameOk && value && void save())}
          placeholder="value (write-only)"
          type="password"
        />
        <Button className="whitespace-nowrap" onClick={() => void save()} disabled={busy || !nameOk || !value}>
          {busy ? 'Saving' : 'Set secret'}
        </Button>
      </div>
      {name && !nameOk && <p className="text-xs text-muted">UPPER_SNAKE, 2–64 chars, starts with a letter.</p>}
      {err && <p className="text-xs text-danger">{err}</p>}
    </div>
  )
}
