// Bumps: the things this instance still needs, said where they actually bite.
//
// There is no setup wizard on purpose. A gate before the product replaces one
// wall with a nicer wall, and the wall is the problem — a new operator's first
// act is entering a provider key, which is itself a secret write, so a broken
// secretbox used to make the app unusable behind a message about a root secret
// they had never heard of.
//
// So: nothing blocks. A missing provider key stops you CHATTING, not using the
// app, and the fix is offered in the same place the gap appears.
//
// Every bump here follows two rules. It appears only on a RESOLVED gap — a
// failed read is never rendered as "you have not configured this", because
// accusing an operator of a gap they do not have sends them to fix something
// that was never broken. And it says what the gap costs, not just that it
// exists.

import { useEffect, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { useSession } from '@/lib/session'
import { useModels } from '@/lib/muse'
import { addEndpoint, patchEndpoint, PROVIDER_PRESETS } from '@/lib/models'
import { getJson } from '@/lib/fetch-json'
import { useSecretHealth } from '@/lib/secrets'
import { useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'

// ── Unreadable secrets ───────────────────────────────────────────────────────

const DISMISS_KEY = 'talaria.secrets-banner-dismissed'

/** App-wide, admin-only, dismissible. Unreadable secrets fail at USE time — a
 *  chat that will not start, a Drive sync that stops, an SMTP send that
 *  silently does not — so without this an operator learns about it from a
 *  confused colleague days later. */
export function UnreadableSecretsBanner() {
  const session = useSession()
  const isAdmin = session.data?.role === 'admin'
  const { data } = useSecretHealth(isAdmin)
  const [dismissed, setDismissed] = useState<string | null>(null)

  // Read after mount: the server has no localStorage, and rendering the banner
  // on the server only to remove it on hydration is a visible flash.
  useEffect(() => {
    setDismissed(localStorage.getItem(DISMISS_KEY))
  }, [])

  if (!data) return null
  const broken = data.root.state === 'unreadable' || data.root.state === 'absent'
  if (!broken && data.counts.unreadable === 0) return null

  // The situation, not a boolean. Dismissing "3 unreadable" must not also
  // silence "9 unreadable" next week — a dismissal is "I have seen THIS", and
  // a bigger breakage is a different this.
  const situation = `${data.root.state}:${data.counts.unreadable}`
  if (dismissed === situation) return null

  const dismiss = () => {
    localStorage.setItem(DISMISS_KEY, situation)
    setDismissed(situation)
  }

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-danger/40 bg-danger/5 px-4 py-2">
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">
        {data.root.state === 'absent' ? 'No encryption root' : 'Secrets unreadable'}
      </span>
      <span className="min-w-0 flex-1 text-xs text-muted">
        {data.root.state === 'absent'
          ? 'TALARIA_SECRET_KEY is not set, so nothing new can be sealed — provider keys and connections cannot be saved.'
          : data.root.state === 'unreadable'
            ? `This instance cannot unwrap its data key, so ${data.counts.unreadable} stored secret${data.counts.unreadable === 1 ? '' : 's'} cannot be read. Restoring the original root secret recovers them.`
            : `${data.counts.unreadable} stored secret${data.counts.unreadable === 1 ? ' is' : 's are'} sealed with a key this instance no longer has.`}
      </span>
      <Link
        to="/admin"
        search={{ tab: 'secrets' as const }}
        className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-danger underline-offset-2 hover:underline"
      >
        Review secrets
      </Link>
      <button
        type="button"
        onClick={dismiss}
        // Dismissible on purpose: this is a state to fix, not a modal to fight.
        // An operator mid-recovery should not have to argue with the banner.
        className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim transition-colors hover:text-fg"
      >
        Dismiss
      </button>
    </div>
  )
}

// ── No model configured ──────────────────────────────────────────────────────

const KEYED_PRESETS = PROVIDER_PRESETS.filter((p) => p.class === 'cloud' && p.apiKeyEnv)

/** Shown on the surfaces that cannot work without a model. Admins get the
 *  field right here; everyone else gets told who to ask, because they cannot
 *  fix it and a form they cannot submit is worse than a sentence. */
export function NoModelBump({ className }: { className?: string }) {
  const session = useSession()
  const models = useModels()
  const qc = useQueryClient()
  const [preset, setPreset] = useState(KEYED_PRESETS[0]?.key ?? '')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)

  // A resolved empty catalog is the gap. Pending is not, and neither is a
  // failed read — /api/models answering 500 must not read as "no models".
  if (models.isPending || models.isError || (models.data?.models.length ?? 0) > 0) return null

  const isAdmin = session.data?.role === 'admin'

  const add = async () => {
    const p = KEYED_PRESETS.find((x) => x.key === preset)
    if (!p || !key.trim()) return
    setBusy(true)
    setMsg(null)
    try {
      const created = await addEndpoint({
        name: p.key,
        provider: p.provider,
        baseUrl: p.baseUrl ?? null,
        class: p.class,
        apiKeyEnv: p.apiKeyEnv ?? null,
        apiKey: key.trim(),
      })
      if (created.error || !created.id) return setMsg(created.error ?? 'could not add that provider')
      setKey('')
      // An endpoint with no models registered serves nothing, so adding a key
      // alone would leave the picker just as empty and the bump would look
      // broken. Register what the provider actually reports — live, never a
      // baked-in list — and say how many, with where to curate them.
      const avail = await getJson<{ models: string[]; note?: string }>(
        `/api/fleet/endpoints/${created.id}/available`,
      ).catch(() => ({ models: [] as string[], note: 'could not reach the provider' }))
      if (avail.models.length) {
        const patched = await patchEndpoint(created.id, { models: avail.models })
        setMsg(
          patched.error
            ? `Added ${p.label}, but its models could not be registered: ${patched.error}`
            : `${p.label} added with ${avail.models.length} model${avail.models.length === 1 ? '' : 's'}. Curate them on Models.`,
        )
      } else {
        setMsg(`${p.label} added, but it returned no models${avail.note ? ` — ${avail.note}` : ''}. Check the key on Models.`)
      }
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['gateway-models'] }),
        qc.invalidateQueries({ queryKey: ['fleet-endpoints'] }),
      ])
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className={cn('rounded-md border border-line bg-raised/40 p-4', className)}>
      <p className="mb-1 text-sm text-fg">No model is configured yet.</p>
      <p className="mb-3 text-xs leading-relaxed text-muted">
        {isAdmin
          ? 'Chat, Plan and Research all route through a provider. Add a key and this instance can answer — everything else keeps working either way.'
          : 'Chat, Plan and Research need a model provider. An admin can add one on Models; the rest of Talaria works without it.'}
      </p>
      {isAdmin && (
        <>
          <div className="flex flex-wrap items-end gap-2">
            <Select value={preset} onChange={(e) => setPreset(e.target.value)} className="w-40">
              {KEYED_PRESETS.map((p) => (
                <option key={p.key} value={p.key}>
                  {p.label}
                </option>
              ))}
            </Select>
            <Input
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder="paste the API key"
              autoComplete="off"
              className="min-w-[14rem] flex-1"
            />
            <Button size="sm" onClick={() => void add()} disabled={busy || !key.trim()}>
              {busy ? 'Adding' : 'Add'}
            </Button>
          </div>
          <p className="mt-2 text-xs text-muted">
            {msg ?? (
              <>
                Stored encrypted, never shown again. More providers and per-model curation live on{' '}
                <Link to="/models" className="underline underline-offset-2 hover:text-fg">
                  Models
                </Link>
                .
              </>
            )}
          </p>
        </>
      )}
    </div>
  )
}

// ── No email transport ───────────────────────────────────────────────────────

/** Invites are sent by email. Without a transport an admin can create one and
 *  watch it go nowhere, which looks like a bug in invites. */
export function NoEmailBump({ className }: { className?: string }) {
  const session = useSession()
  // Read from the inventory rather than /api/admin/email, so this bump and the
  // Secrets page can never disagree about whether email is set up.
  const { data } = useSecretHealth(session.data?.role === 'admin')
  if (!data) return null // no answer yet, or the read failed — not a gap
  const email = data.rows.find((r) => r.id.startsWith('setting:email_config:'))
  if (email?.state === 'ok') return null
  return (
    <p className={cn('text-xs text-warning', className)}>
      {email?.state === 'unreadable'
        ? 'The stored email credential cannot be read, so invites will not be delivered — you can still copy an invite link and send it yourself. Replace it under Organization.'
        : 'No email transport is configured, so invites cannot be delivered — you can still copy an invite link and send it yourself. Set one up under Organization.'}
    </p>
  )
}
