<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { AlertTriangle, Eye, EyeOff, Folder, KeyRound, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { getList } from '@/lib/fetch-json'
  import Combobox from '@/components/ui/Combobox.svelte'
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
  let { agentId, agentModel, agentLabel }: { agentId: string; agentModel: string; agentLabel: string } = $props()

  // ── WHAT IT CAN SPEND WITHOUT SEEING ───────────────────────────────────────
  //
  // The section below this one is env vars, and env vars are PLAINTEXT IN THE
  // CONTAINER: every fleet agent runs a harness with a shell, so `echo $NAME`
  // returns the value and it is one step from the model's context and a
  // provider. That is the leak the handle store exists to close, and until now
  // the two lived on different pages with nothing saying they were alternatives.
  //
  // So both are here, in this order, with the safe one first.
  const held = createQuery(() => ({
    queryKey: ['agent-held-handles', agentModel],
    // The `?agent=` form, not the full listing: a credential reaching this agent
    // through a shared FOLDER is invisible in `SecretDoc.grants`, and a page
    // that said "no credentials" about an agent holding four would be worse
    // than no page.
    queryFn: (): Promise<Array<{ name: string; title: string; key: string; label: string; via: 'direct' | 'folder'; folder: string | null }>> =>
      getList(`/api/admin/workspace-secrets?agent=${encodeURIComponent(agentModel)}`, 'held'),
  }))
  const vault = createQuery(() => ({
    queryKey: ['workspace-secrets'],
    queryFn: (): Promise<Array<{ name: string; title: string; grants: string[]; entries: Array<{ key: string; label: string }> }>> =>
      getList('/api/admin/workspace-secrets', 'secrets'),
  }))
  const grantable = $derived((vault.data ?? []).filter((v) => !v.grants.includes(agentModel)))

  let granting = $state(false)
  const setGrant = async (name: string, on: boolean) => {
    granting = true
    await fetch('/api/admin/workspace-secrets', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ action: on ? 'grant' : 'revoke', name, agentModel }),
    }).catch(() => null)
    granting = false
    await qc.invalidateQueries({ queryKey: ['agent-held-handles', agentModel] })
    await qc.invalidateQueries({ queryKey: ['workspace-secrets'] })
  }

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
  <!-- ── SAFE: spends without seeing ────────────────────────────────────────── -->
  <div class="space-y-2">
    <div class="flex items-center gap-2">
      <EyeOff size={14} class="text-muted" aria-hidden="true" />
      <p class="font-sans text-sm text-fg">Credentials it can spend, without ever seeing them</p>
    </div>
    <p class="font-sans text-xs leading-relaxed text-muted">
      {agentLabel} is given a handle. Talaria substitutes the real value at the boundary that spends it — a tool call, a push — so the
      value never enters its context and never reaches a provider. Prefer this whenever the credential only has to be USED.
    </p>

    {#if held.data && held.data.length > 0}
      <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
        {#each held.data as h (h.name + h.key)}
          <li class="flex flex-wrap items-center gap-2 px-3.5 py-2.5">
            <KeyRound size={13} class="shrink-0 text-muted" aria-hidden="true" />
            <span class="font-sans text-sm text-fg">{h.title}</span>
            <span class="font-sans text-xs text-muted">{h.label}</span>
            <code class="font-mono text-[11px] text-accent">«secret:{h.name}»</code>
            {#if h.via === 'folder'}
              <!-- REVOKED FROM THE FOLDER, NOT FROM HERE. Offering a button that
                   cannot do what it says is worse than offering none. -->
              <span class="inline-flex items-center gap-1 font-mono text-[10px] text-ink-dim">
                <Folder size={11} aria-hidden="true" />
                via {h.folder}
              </span>
            {:else}
              <button
                type="button"
                title="Revoke"
                disabled={granting}
                onclick={() => void setGrant(h.name, false)}
                class="ml-auto shrink-0 text-muted transition-colors hover:text-danger"
              >
                <Trash2 size={14} />
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="font-sans text-xs text-muted">None yet.</p>
    {/if}

    {#if grantable.length > 0}
      <Combobox
        options={grantable.map((v) => ({ value: v.name, label: v.title, sub: v.entries.map((e) => e.label).join(', ') }))}
        selected={[]}
        placeholder="Grant a workspace credential"
        disabled={granting}
        onChange={(v) => v[0] && void setGrant(v[0], true)}
      />
    {/if}
  </div>

  <!-- ── UNSAFE-BY-CONSTRUCTION: plaintext in the container ─────────────────── -->
  <div class="flex items-center gap-2 border-t border-line pt-4">
    <Eye size={14} class="text-warning" aria-hidden="true" />
    <p class="font-sans text-sm text-fg">Environment variables it can read</p>
  </div>
  <p class="font-sans text-xs leading-relaxed text-muted">
    <span class="inline-flex items-center gap-1 text-warning"><AlertTriangle size={12} aria-hidden="true" /> {agentLabel} can read these.</span>
    They are written into its container as plaintext and loaded as env vars, so anything running in there — including the shell its harness
    uses — can print them. Use one only when a process inside the container reads the variable itself; if the credential merely needs to be
    spent through a tool, grant a handle above instead.
  </p>
  <QueryState {query} errorTitle="Could not load this agent's secrets" errorVariant="compact">
    {#snippet skeleton()}
      <ul aria-hidden="true" class="divide-y divide-line rounded-lg border border-line">
        {#each [0, 1] as i (i)}
          <li class="flex items-center gap-3 px-3.5 py-3.5">
            <Skeleton class="h-3 w-32 rounded-full" />
            <Skeleton class="h-2.5 w-14 rounded-full" />
            <Skeleton class="ml-auto h-2.5 w-24 rounded-full" />
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
          <li class="flex items-center gap-3 px-3.5 py-2.5 transition-colors hover:dither-fill">
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
