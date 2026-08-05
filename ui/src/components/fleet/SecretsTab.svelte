<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { KeyRound, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { listStagger, slide } from '@/lib/motion'

  interface SecretMeta {
    name: string
    updatedBy: string | null
    updatedAt: string
  }

  // Per-agent secrets: UI-configured env vars, encrypted at rest, write-only
  // (values never come back). Materialized into the agent's env at render — no
  // hand edits to fleet/.env unless you're an advanced deployer who wants to.
  let { agentId }: { agentId: string } = $props()

  const qc = useQueryClient()
  const key = () => ['agent-secrets', agentId]
  // GET is 200 `{ secrets }` or 403 — there is no 404 here, so EVERY non-2xx is
  // a failure. It used to answer `[]`, which rendered "No secrets": an operator
  // reading that would go add the token that is already there.
  const query = createQuery(() => ({
    queryKey: key(),
    queryFn: (): Promise<SecretMeta[]> => getList<SecretMeta>(`/api/fleet/agents/${agentId}/secrets`, 'secrets'),
  }))
  let name = $state('')
  let value = $state('')
  let busy = $state(false)
  let err = $state<string | null>(null)
  const nameOk = $derived(/^[A-Z][A-Z0-9_]{1,63}$/.test(name))

  const save = async () => {
    busy = true
    err = null
    try {
      const r = await fetch(`/api/fleet/agents/${agentId}/secrets`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name, value }),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        err = j.error ?? 'could not save'
        return
      }
      name = ''
      value = ''
      await qc.invalidateQueries({ queryKey: key() })
    } finally {
      busy = false
    }
  }

  const remove = async (n: string) => {
    if (!(await confirm({ title: 'Remove secret', message: `Remove ${n}? The agent loses it on its next start.`, confirmLabel: 'Remove', danger: true }))) return
    await fetch(`/api/fleet/agents/${agentId}/secrets?name=${encodeURIComponent(n)}`, { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: key() })
  }
</script>

{#snippet keyIcon()}<KeyRound size={22} />{/snippet}

<div class="space-y-4">
  <p class="font-sans text-xs leading-relaxed text-muted">
    Environment variables just for this agent: a vendor token, a service key. Encrypted at rest, write-only here,
    and injected into the container when it starts from Talaria (Start recreates it with the latest values).
  </p>
  <QueryState {query} errorTitle="Could not load this agent's secrets" errorVariant="compact">
    {#snippet skeleton()}
      <ul aria-hidden="true" class="divide-y divide-line rounded-lg border border-line">
        {#each [0, 1] as i (i)}
          <li class="flex items-center gap-3 px-3.5 py-3.5">
            <Skeleton class="h-3 w-32 rounded-full" delay={i * 0.12} />
            <Skeleton class="h-2.5 w-14 rounded-full" delay={i * 0.12 + 0.12} />
            <Skeleton class="ml-auto h-2.5 w-24 rounded-full" delay={i * 0.12 + 0.24} />
          </li>
        {/each}
      </ul>
    {/snippet}
    {#snippet empty()}
      <EmptyState icon={keyIcon} title="No secrets" hint="Everything it needs comes from the shared platform env." />
    {/snippet}
    {#snippet children(secrets)}
      <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
        {#each secrets as s (s.name)}
          <li class="flex items-center gap-3 px-3.5 py-2.5 transition-colors hover:bg-hover">
            <code class="font-mono text-sm text-fg">{s.name}</code>
            <span class="font-mono text-xs text-muted">••••••••</span>
            <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">
              {s.updatedBy ?? 'unknown'} · {relativeTime(s.updatedAt)}
            </span>
            <button
              type="button"
              title="Remove"
              onclick={() => void remove(s.name)}
              class="shrink-0 text-muted transition-colors hover:text-danger"
            >
              <Trash2 size={14} />
            </button>
          </li>
        {/each}
      </ul>
    {/snippet}
  </QueryState>
  <div class="grid gap-3 sm:grid-cols-[1fr_1fr_auto]">
    <Input
      value={name}
      oninput={(e) => (name = e.currentTarget.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
      onkeydown={submitOnEnter(() => !busy && nameOk && value && void save())}
      placeholder="FIGMA_TOKEN"
    />
    <Input
      bind:value
      onkeydown={submitOnEnter(() => !busy && nameOk && value && void save())}
      placeholder="value (write-only)"
      type="password"
    />
    <Button class="whitespace-nowrap" onclick={() => void save()} disabled={busy || !nameOk || !value}>
      {busy ? 'Saving' : 'Set secret'}
    </Button>
  </div>
  {#if name && !nameOk}<p class="text-xs text-muted">UPPER_SNAKE, 2–64 chars, starts with a letter.</p>{/if}
  {#if err}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{err}</p>{/if}
</div>
