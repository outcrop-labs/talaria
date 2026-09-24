<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { delJson, errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { useFleet, relativeTime } from '@/lib/fleet'
  import { patchAgentMeta, type AgentDef, type ModelTarget } from '@/lib/fleet-defs'
  import Stat from './Stat.svelte'
  import TemplateBindings from './TemplateBindings.svelte'
  import WorkbenchControl from './WorkbenchControl.svelte'

  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const cfg = $derived(def.latest?.config)
  const fleetQuery = useFleet()
  const stat = $derived(fleetQuery.data?.agents.find((a) => a.id === def.model))
  // svelte-ignore state_referenced_locally -- reason: seed-once meta drafts; the refetch after save must not clobber in-progress edits
  let role = $state(def.role ?? '')
  const saveRole = async () => {
    if (role.trim() === (def.role ?? '')) return
    await patchAgentMeta(def.id, { role: role.trim() || null })
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
  // Send address for ORG agents only — a personal assistant sends as its
  // owner, where no alias applies. Empty = the derived org plus-address
  // (org+slug@domain); an override must be an address Gmail will accept
  // (a verified send-as on the org account).
  const isOrg = $derived(def.ownerUserId === null)
  // svelte-ignore state_referenced_locally -- reason: seed-once meta drafts; the refetch after save must not clobber in-progress edits
  let alias = $state(def.emailAlias ?? '')
  const saveAlias = async () => {
    if (alias.trim() === (def.emailAlias ?? '')) return
    await patchAgentMeta(def.id, { emailAlias: alias.trim() || null })
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }

  type GoogleIdentity = {
    principalKind: 'owner' | 'org' | 'agent'
    principalUserId: string | null
    connected: boolean
    email: string | null
    legacyOwnerUserId: string | null
  }
  const googleQuery = createQuery(() => ({
    queryKey: ['agent-google', def.id],
    enabled: isAdmin,
    queryFn: (): Promise<GoogleIdentity> => getJson<GoogleIdentity>(`/api/fleet/defs/${def.id}/google`),
  }))
  let kind = $state<'owner' | 'org' | 'agent'>('org')
  let principalUserId = $state('')
  let googleError = $state('')
  let googleBusy = $state(false)
  let googleSeeded = $state(false)
  $effect(() => {
    const row = googleQuery.data
    if (!row || googleSeeded) return
    kind = row.principalKind
    principalUserId = row.principalUserId ?? ''
    googleSeeded = true
  })
  const savePrincipal = async () => {
    googleError = ''
    googleBusy = true
    try {
      const saved = await putJson<GoogleIdentity>(`/api/fleet/defs/${def.id}/google`, {
        principalKind: kind,
        principalUserId: kind === 'owner' ? principalUserId.trim() || null : null,
      })
      kind = saved.principalKind
      principalUserId = saved.principalUserId ?? ''
      await qc.invalidateQueries({ queryKey: ['agent-google', def.id] })
    } catch (e) {
      googleError = errorMessage(e)
    } finally {
      googleBusy = false
    }
  }
  const disconnectGoogle = async () => {
    googleError = ''
    googleBusy = true
    try {
      const saved = await delJson<GoogleIdentity>(`/api/fleet/defs/${def.id}/google`)
      kind = saved.principalKind
      principalUserId = saved.principalUserId ?? ''
      await qc.invalidateQueries({ queryKey: ['agent-google', def.id] })
    } catch (e) {
      googleError = errorMessage(e)
    } finally {
      googleBusy = false
    }
  }
</script>

<!-- Model identity is chrome — mono chip, radius 6, class in signal color. -->
{#snippet targetChip(t: ModelTarget, name?: string)}
  {@const local = /inference|vllm|ollama|spark|local/.test(t.endpoint)}
  <span class="inline-flex items-center gap-1.5 rounded-md border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em]">
    {#if name}<span class="font-medium uppercase text-fg">{name}</span>{/if}
    <span class="text-muted">{t.model}</span>
    <span class={cn('uppercase', local ? 'text-success' : 'text-accent')}>{local ? 'self-hosted' : 'cloud'}</span>
  </span>
{/snippet}

<!-- A Stat cell's shape while its query is in flight: label bar + value bar. -->
{#snippet statSkeleton()}
  <div class="space-y-1.5">
    <Skeleton class="h-2.5 w-20 rounded-full" />
    <Skeleton class="h-3.5 w-24 rounded-full" />
  </div>
{/snippet}

<div class="space-y-4 text-sm">
  <!-- Editable role — the human-readable title shown on the roster. -->
  <div>
    <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</div>
    {#if isAdmin}
      <Input size="sm" bind:value={role} onblur={() => void saveRole()} placeholder="e.g. Support Lead" class="max-w-xs" />
    {:else}
      <div class="text-fg">{def.role ?? '—'}</div>
    {/if}
  </div>
  {#if isOrg}
    <!-- Send address — empty means the derived org plus-address for the slug. -->
    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Email alias</div>
      {#if isAdmin}
        <Input size="sm" bind:value={alias} onblur={() => void saveAlias()} placeholder={`auto: org+${def.slug}`} class="max-w-xs" />
        <div class="mt-1 text-xs text-muted">
          Empty sends from the org account’s plus-address for this agent ({def.slug}). Set an address only if it is a verified send-as on the org Google account.
        </div>
      {:else}
        <div class="text-fg">{def.emailAlias ?? `org+${def.slug}`}</div>
      {/if}
    </div>
  {/if}
  {#if isAdmin}
    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Google identity</div>
      {#if googleQuery.isLoading}
        <Skeleton class="h-8 w-64" />
      {:else if googleQuery.isError}
        <div class="text-xs text-danger">{errorMessage(googleQuery.error)}</div>
      {:else}
        <div class="flex flex-wrap items-center gap-2">
          <select bind:value={kind} class="rounded-md border border-line bg-surface px-2 py-1 text-xs text-fg">
            <option value="org">Shared org account</option>
            <option value="owner">A person's account</option>
            <option value="agent">This agent's own account</option>
          </select>
          {#if kind === 'owner'}
            <Input size="sm" bind:value={principalUserId} placeholder="owner user id" class="max-w-xs font-mono" />
          {/if}
          <Button size="sm" disabled={googleBusy} onclick={() => void savePrincipal()}>Save</Button>
        </div>
        <div class="mt-1 text-xs text-muted">
          {#if googleQuery.data?.principalKind === 'agent'}
            {googleQuery.data.connected ? `Connected as ${googleQuery.data.email ?? 'a Google account'}.` : 'Not connected. Writes refuse until an admin connects this agent’s own Google account.'}
          {:else if googleQuery.data?.principalKind === 'owner'}
            {googleQuery.data.connected ? `Owner’s Google is connected${googleQuery.data.email ? ` (${googleQuery.data.email})` : ''}.` : 'That person has not connected Google. This agent will not fall back to the org account.'}
          {:else}
            Fleet default: the shared org account. An admin approves outbound writes.
          {/if}
        </div>
        {#if kind === 'agent'}
          <div class="mt-2 flex items-center gap-2">
            <a href={`/api/fleet/defs/${def.id}/google/connect`} class="rounded-md border border-line bg-surface px-2.5 py-1 text-xs text-fg hover:bg-raised">Connect Google</a>
            {#if googleQuery.data?.connected}
              <Button size="sm" variant="ghost" disabled={googleBusy} onclick={() => void disconnectGoogle()}>Disconnect</Button>
            {/if}
          </div>
        {/if}
        {#if googleError}<div class="mt-1 text-xs text-danger">{googleError}</div>{/if}
      {/if}
    </div>
  {/if}
  <!-- Workbench — THE sandbox setting: off / auto (fit rules) / on. -->
  <WorkbenchControl {def} {isAdmin} />
  <div class="grid grid-cols-2 gap-3">
    <Stat label="Model id" value={def.model} />
    <Stat label="Department" value={def.department} />
    <Stat label="Management" value="Talaria-managed" />
    <Stat label="Version" value={`v${def.currentVersion}`} />
    {#if fleetQuery.isLoading}
      <!-- The usage cells land late and grow the grid — hold their spots. -->
      {@render statSkeleton()}
      {@render statSkeleton()}
      {@render statSkeleton()}
    {:else}
      {#if stat}<Stat label="Conversations" value={String(stat.conversations)} />{/if}
      {#if stat}<Stat label="Messages" value={String(stat.messages)} />{/if}
      {#if stat?.lastUsed}<Stat label="Last used" value={relativeTime(stat.lastUsed)} />{/if}
    {/if}
  </div>
  <div>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Models</div>
    <div class="flex flex-wrap items-center gap-2">
      {#if cfg?.main}{@render targetChip(cfg.main, 'main')}{/if}
      {#each cfg?.aliases ?? [] as a (a.name)}{@render targetChip(a, a.name)}{/each}
      {#if !cfg?.main}<span class="text-xs text-muted">no config yet</span>{/if}
    </div>
    {#if cfg?.fallbacks?.length}
      <div class="mt-2 text-xs text-muted">↯ fallback: {cfg.fallbacks.map((f) => f.model).join(' → ')}</div>
    {/if}
  </div>
  {#if cfg?.mcpServers?.length}
    <Stat label="MCP" value={cfg.mcpServers.join(', ')} />
  {/if}
  <TemplateBindings {def} {isAdmin} />
</div>
