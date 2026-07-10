import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { EmptyState } from '@/components/ui/empty-state'
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
  const { data: secrets = [], isLoading } = useQuery({
    queryKey: key,
    queryFn: async (): Promise<SecretMeta[]> => {
      const r = await fetch(`/api/fleet/agents/${agentId}/secrets`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { secrets: SecretMeta[] }).secrets
    },
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
      <p className="text-xs leading-relaxed text-muted">
        Environment variables just for this agent — a vendor token, a service key. Encrypted at rest, write-only here,
        and injected into the container when it starts from Talaria (Start recreates it with the latest values).
      </p>
      {isLoading ? null : secrets.length === 0 ? (
        <EmptyState icon={<KeyRound size={22} />} title="No secrets" hint="Everything it needs comes from the shared platform env." />
      ) : (
        <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
          {secrets.map((s) => (
            <li key={s.name} className="flex items-center gap-3 px-3.5 py-2.5">
              <code className="text-sm text-fg">{s.name}</code>
              <span className="text-xs text-muted">••••••••</span>
              <span className="ml-auto shrink-0 text-[11px] text-muted">
                {s.updatedBy ?? 'unknown'} · {relativeTime(s.updatedAt)}
              </span>
              <button
                type="button"
                title="Remove"
                onClick={() => void remove(s.name)}
                className="shrink-0 text-muted transition-colors hover:text-[color:var(--theme-danger)]"
              >
                <Trash2 size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto]">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
          placeholder="FIGMA_TOKEN"
        />
        <Input value={value} onChange={(e) => setValue(e.target.value)} placeholder="value (write-only)" type="password" />
        <Button className="whitespace-nowrap" onClick={() => void save()} disabled={busy || !nameOk || !value}>
          {busy ? 'Saving' : 'Set secret'}
        </Button>
      </div>
      {name && !nameOk && <p className="text-xs text-muted">UPPER_SNAKE, 2–64 chars, starts with a letter.</p>}
      {err && <p className="text-xs text-[color:var(--theme-danger)]">{err}</p>}
    </div>
  )
}
