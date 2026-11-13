<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { AlertTriangle } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { delJson, errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { useFleet, relativeTime } from '@/lib/fleet'
  import { patchAgentMeta, type AgentDef, type ModelTarget } from '@/lib/fleet-defs'
  import DeveloperAgentControl from './DeveloperAgentControl.svelte'
  import MaxChildrenControl from './MaxChildrenControl.svelte'
  import PinnedSessionsControl from './PinnedSessionsControl.svelte'
  import TemplateBindings from './TemplateBindings.svelte'

  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  const cfg = $derived(def.latest?.config)
  const share = $derived(def.departmentShare ?? null)
  const sharedWith = $derived((share?.others ?? []).map((o) => o.displayName).join(', '))
  const sharedOr = $derived((share?.others ?? []).map((o) => o.displayName).join(' or '))
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
    enabled: canManage,
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
      googleFlash.flash()
    } catch (e) {
      googleError = errorMessage(e)
    } finally {
      googleBusy = false
    }
  }
  // This choice decides whose Google account the agent acts through, so a
  // slip on the dropdown must not apply it: the row keeps its own Save, shown
  // while the choice differs from the stored one.
  const googleFlash = useSavedFlash()
  const googleDirty = $derived.by(() => {
    const g = googleQuery.data
    if (!g || !googleSeeded) return false
    return kind !== g.principalKind || (kind === 'owner' && principalUserId.trim() !== (g.principalUserId ?? ''))
  })
  const googleStatus = $derived.by(() => {
    const g = googleQuery.data
    if (!g) return ''
    if (g.principalKind === 'agent')
      return g.connected ? `Connected as ${g.email ?? 'a Google account'}.` : 'Not connected. It cannot write until an admin connects its account.'
    if (g.principalKind === 'owner')
      return g.connected
        ? `That person's Google is connected${g.email ? ` (${g.email})` : ''}.`
        : 'That person has not connected Google. The agent does not fall back to the org account.'
    return 'The shared org account. An admin approves outbound writes.'
  })
  const isLocal = (t: ModelTarget) => /inference|vllm|ollama|spark|local/.test(t.endpoint)
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

{#snippet target(t: ModelTarget, name?: string)}
  <span class="inline-flex min-w-0 items-center gap-2">
    {#if name}<span class="font-sans text-[13px] leading-5 text-muted">{name}</span>{/if}
    <span class="truncate font-mono text-xs text-fg">{t.model}</span>
    <Chip>{isLocal(t) ? 'Self-hosted' : 'Cloud'}</Chip>
  </span>
{/snippet}

{#snippet fact(label: string, value: string, mono = false)}
  <dt class="font-sans text-[13px] leading-5 text-muted">{label}</dt>
  <dd class={mono ? 'min-w-0 truncate font-mono text-xs leading-5 text-fg' : 'min-w-0 truncate font-sans text-[13px] leading-5 text-fg tabular-nums'} title={value}>{value}</dd>
{/snippet}

<div class="space-y-10">
  <!-- A department another agent holds too (a pair from before a department
       became one agent's). It names one container and one memory volume, so
       the render runs the older agent there and leaves the other out — said
       here, on the page of each, with who runs and what to do. -->
  {#if share}
    <div class="flex items-start gap-3 rounded-lg border border-warning/40 bg-warning/5 px-5 py-4" data-testid="department-share">
      <AlertTriangle size={16} class="mt-0.5 shrink-0 text-warning" />
      <div class="min-w-0">
        <p class="font-sans text-sm font-medium leading-5 text-fg">Shares the department “{def.department}”</p>
        <p class="mt-1 font-sans text-[13px] leading-5 text-muted">
          {#if share.runs}
            This agent runs in the department’s container and keeps its memory.
            {sharedWith} {share.others.length === 1 ? 'holds' : 'hold'} the same department and {share.others.length === 1 ? 'is' : 'are'} left out of the fleet.
            If the container still runs {sharedOr}, restart this agent on a fresh container to put it back.
          {:else if share.runner}
            {share.runner.displayName} runs in this department’s container, so this agent has none and is left out of the fleet:
            it cannot be started or stopped, its memory is not read or written here, and a saved config is not applied.
            If the container still runs this agent, restart {share.runner.displayName} on a fresh container.
          {:else}
            {sharedWith} {share.others.length === 1 ? 'holds' : 'hold'} the same department, and none of them is running.
          {/if}
          A department is one container and one memory. Retire and delete one of the agents to untie it.
        </p>
      </div>
    </div>
  {/if}

  <SettingsSection title="Identity">
    <SettingsGroup>
      <SettingsRow label="Role" hint="The title shown on the roster." as={canManage ? 'label' : 'div'}>
        {#if canManage}
          <Input size="sm" bind:value={role} onblur={() => void saveRole()} placeholder="Support lead" class="w-[280px]" />
        {:else}
          <span class="font-sans text-[13px] leading-5 text-fg">{def.role ?? 'None'}</span>
        {/if}
      </SettingsRow>
      {#if isOrg}
        <!-- Empty means the derived org plus-address for the slug. -->
        <SettingsRow
          label="Email alias"
          hint="Leave empty to send from the org address for this agent."
          info="An address set here must be a verified send-as on the org Google account."
          as={canManage ? 'label' : 'div'}
        >
          {#if canManage}
            <Input size="sm" bind:value={alias} onblur={() => void saveAlias()} placeholder={`org+${def.slug}`} class="w-[280px]" />
          {:else}
            <span class="font-mono text-xs text-fg">{def.emailAlias ?? `org+${def.slug}`}</span>
          {/if}
        </SettingsRow>
      {/if}
      {#if canManage}
        <SettingsRow label="Google account">
          {#snippet hint()}
            {#if googleError}<span class="text-danger">{googleError}</span>{:else if googleDirty}Save to apply this change.{:else}{googleStatus}{/if}
          {/snippet}
          {#if googleQuery.isLoading}
            <Skeleton class="h-8 w-[280px]" />
          {:else if googleQuery.isError}
            <span class="font-sans text-[13px] leading-5 text-danger">{errorMessage(googleQuery.error)}</span>
          {:else}
            {#if googleFlash.saved && !googleError && !googleDirty}
              <span class="font-sans text-[13px] leading-5 text-success">Saved</span>
            {/if}
            {#if kind === 'agent' && !googleDirty}
              {#if googleQuery.data?.connected}
                <DangerLink onClick={() => void disconnectGoogle()} disabled={googleBusy}>Disconnect Google</DangerLink>
              {/if}
              <a href={`/api/fleet/defs/${def.id}/google/connect`} class={buttonClasses({ variant: 'outline', size: 'sm', voice: 'sans' })}>Connect Google</a>
            {/if}
            {#if kind === 'owner'}
              <Input
                size="sm"
                bind:value={principalUserId}
                onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && googleDirty && void savePrincipal()}
                placeholder="User id"
                aria-label="The person's user id"
                disabled={googleBusy}
                class="w-36 font-mono"
              />
            {/if}
            <Select
              size="sm"
              bind:value={kind}
              disabled={googleBusy}
              aria-label="Google account"
              class={kind === 'owner' ? 'w-[200px]' : 'w-[280px]'}
            >
              <option value="org">Shared org account</option>
              <option value="owner">A person's account</option>
              <option value="agent">This agent's own account</option>
            </Select>
            {#if googleDirty}
              <Button variant="outline" size="md" disabled={googleBusy} onclick={() => void savePrincipal()}>Save</Button>
            {/if}
          {/if}
        </SettingsRow>
      {/if}
    </SettingsGroup>
  </SettingsSection>

  <SettingsSection title="Behavior">
    <SettingsGroup>
      <!-- Developer agent: one switch for sandbox, Oh My Pi, Workbench tools. -->
      <DeveloperAgentControl {def} {canManage} />
      <!-- Pinned sessions: the conversation is the Hermes session. -->
      <PinnedSessionsControl {def} {canManage} />
      <!-- The subagent cap: how many helpers this agent may run at once. -->
      <MaxChildrenControl {def} {canManage} />
      <TemplateBindings {def} {canManage} />
    </SettingsGroup>
  </SettingsSection>

  <SettingsSection title="Details">
    <SettingsGroup>
      <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4 md:grid-cols-[10rem_minmax(0,1fr)_10rem_minmax(0,1fr)]">
        {@render fact('Agent id', def.model, true)}
        {@render fact('Department', def.department, true)}
        {@render fact('Run by', def.managed ? 'Talaria' : 'Outside Talaria')}
        {@render fact('Version', String(def.currentVersion))}
        {#if fleetQuery.isLoading}
          <!-- The usage facts land late and grow the grid: hold their spots. -->
          {#each ['Conversations', 'Messages'] as label (label)}
            <dt class="font-sans text-[13px] leading-5 text-muted">{label}</dt>
            <dd><Skeleton class="h-3.5 w-16 rounded-full" /></dd>
          {/each}
        {:else if stat}
          {@render fact('Conversations', stat.conversations.toLocaleString())}
          {@render fact('Messages', stat.messages.toLocaleString())}
          {#if stat.lastUsed}{@render fact('Last used', relativeTime(stat.lastUsed))}{/if}
        {/if}
      </dl>
      <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4">
        <dt class="font-sans text-[13px] leading-5 text-muted">Main model</dt>
        <dd class="min-w-0">
          {#if cfg?.main}{@render target(cfg.main)}{:else}<span class="font-sans text-[13px] leading-5 text-muted">Not configured yet</span>{/if}
        </dd>
        {#if cfg?.aliases?.length}
          <dt class="font-sans text-[13px] leading-5 text-muted">Model tiers</dt>
          <dd class="flex min-w-0 flex-wrap gap-x-5 gap-y-1">
            {#each cfg.aliases as a (a.name)}{@render target(a, a.name)}{/each}
          </dd>
        {/if}
        {#if cfg?.fallbacks?.length}
          <dt class="font-sans text-[13px] leading-5 text-muted">Fallbacks</dt>
          <dd class="min-w-0 font-mono text-xs leading-5 text-fg">{cfg.fallbacks.map((f) => f.model).join(', then ')}</dd>
        {/if}
        {#if cfg?.mcpServers?.length}
          <dt class="font-sans text-[13px] leading-5 text-muted">MCP servers</dt>
          <dd class="min-w-0 font-mono text-xs leading-5 text-fg">{cfg.mcpServers.join(', ')}</dd>
        {/if}
      </dl>
    </SettingsGroup>
  </SettingsSection>
</div>
