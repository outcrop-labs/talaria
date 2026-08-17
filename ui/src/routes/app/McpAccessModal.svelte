<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { slide } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import { useUsers } from '@/lib/users'
  import McpAccessRow from './McpAccessRow.svelte'
  import McpAddPickerButton from './McpAddPickerButton.svelte'
  import { ALL_TOOLS, NO_ACCESS, patchServer, resolveScopePick, type McpServerRow } from './mcp'

  // WHO MAY USE THIS SERVER — its own dialog rather than a third section of the
  // card. Access is a table that grows with the org: one row per agent and one
  // per person, each with a tool-subset control. Inlined, every card on the page
  // carried the whole governance surface open at all times, so the registry read
  // as a wall of controls and the thing you came to check — what this server IS
  // and whether it is connected — was the smallest part of it.
  //
  // The card keeps the SUMMARY (it is a fact about the server, and cheap), and
  // this holds the editing.
  let { server: s, onClose }: { server: McpServerRow; onClose: () => void } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  // Access RULES are shown per person by name. Defaulted to `[]` a failed
  // directory read rendered each existing rule as a truncated raw id and the
  // "add a person" picker as empty — an access table that looks like it is
  // about strangers, on a server that grants tool use.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const users = $derived(usersList.rows)
  let error = $state<string | null>(null)

  const patch = async (body: unknown) => {
    error = null
    const e = await patchServer(s.id, body)
    if (e) error = e
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
  }

  const agentOptions = $derived((fleetQuery.data?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))
  const agentLabel = (model: string) => agentOptions.find((o) => o.value === model)?.label ?? model
  const toolOptions = $derived(s.tools.map((t) => ({ value: t.name, label: t.name })))
  const userLabel = (id: string) => {
    const u = users.find((x) => x.id === id)
    return u?.name ?? u?.email ?? id.slice(0, 8)
  }
</script>

{#snippet title()}
  <span class="flex items-center gap-2">
    Access — {s.label}
    <InfoTip text="Which agents carry this server, and which people may exercise it through agents acting for them. Tool cells narrow a row to a subset; empty = every tool. The gateway enforces all of it." />
  </span>
{/snippet}

<Modal open {onClose} {title} width="max-w-2xl">
  <div class="space-y-5">
  <!-- Agents -->
  <div>
    <div class="flex items-center gap-3 pb-1">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agents</span>
      {#if s.builtin}
        <span class="text-xs text-muted">every agent — rows below narrow individual agents</span>
      {:else}
        <Checkbox
          checked={s.allAgents}
          onChange={(checked) => void patch({ allAgents: checked })}
          label="all agents"
          title="Every enabled agent carries this server; rows below become per-agent tool overrides"
        />
      {/if}
      <span class="flex-1"></span>
      <McpAddPickerButton
        title={s.allAgents ? 'Add a per-agent tool override' : 'Add an agent'}
        placeholder="Search agents"
        options={agentOptions.filter((o) => !s.assignments.some((a) => a.agentModel === o.value))}
        onPick={(m) => void patch({ assign: { agentModel: m, tools: null } })}
      />
    </div>
    {#if s.assignments.length === 0}
      <EmptyState
        variant="inline"
        class="px-1.5 py-1 text-muted/70"
        title={s.allAgents ? 'Every enabled agent, every tool. Add a row to narrow one agent.' : 'No agents yet — add one, or check “all agents”.'}
      />
    {:else}
      <div class="space-y-0.5">
        {#if s.allAgents}
          <div class="px-1.5 pb-0.5 text-[11px] text-muted/70">Every enabled agent carries this server; these rows narrow individual agents.</div>
        {/if}
        {#each s.assignments as a (a.agentModel)}
          <McpAccessRow
            name={agentLabel(a.agentModel)}
            onRemove={() => void patch({ unassign: a.agentModel })}
            removeTitle="Remove this agent"
          >
            {#snippet tools()}
              <Combobox
                options={[{ value: ALL_TOOLS, label: 'All tools' }, ...toolOptions]}
                selected={a.tools ?? [ALL_TOOLS]}
                onChange={(sel) => {
                  const next = resolveScopePick(sel, { denied: false, tools: a.tools })
                  void patch({ assign: { agentModel: a.agentModel, tools: next.tools } })
                }}
                multiple
                size="sm"
                placeholder="All tools"
                class="w-full"
              />
            {/snippet}
          </McpAccessRow>
        {/each}
      </div>
    {/if}
  </div>

  <!-- People -->
  <div class="mt-3 border-t border-line-subtle pt-2.5">
    <div class="flex items-center gap-2 pb-1">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People</span>
      <InfoTip text="No rules = everyone with an assigned agent may use it. A rule narrows one person to specific tools, or denies them outright." />
      <span class="flex-1"></span>
      <McpAddPickerButton
        title="Add a person rule"
        placeholder="Search people"
        options={users
          .filter((u) => !s.userAccess.some((r) => r.userId === u.id))
          .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id.slice(0, 8), sub: u.email ?? undefined }))}
        onPick={(id) => void patch({ userAccess: { userId: id, allowed: true, tools: null } })}
      />
    </div>
    {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
    {#if s.userAccess.length === 0}
      <EmptyState variant="inline" class="px-1.5 py-1 text-muted/70" title="Everyone with an assigned agent may use it." />
    {:else}
      <div class="space-y-0.5">
        {#each s.userAccess as ua (ua.userId)}
          <McpAccessRow
            name={userLabel(ua.userId)}
            dim={!ua.allowed}
            onRemove={() => void patch({ userAccess: { userId: ua.userId, allowed: null, tools: null } })}
            removeTitle="Remove this rule (back to default access)"
          >
            {#snippet tools()}
              <Combobox
                options={[
                  { value: ALL_TOOLS, label: 'All tools' },
                  { value: NO_ACCESS, label: 'No access' },
                  ...toolOptions,
                ]}
                selected={ua.allowed ? (ua.tools ?? [ALL_TOOLS]) : [NO_ACCESS]}
                onChange={(sel) => {
                  const next = resolveScopePick(sel, { denied: !ua.allowed, tools: ua.tools })
                  void patch({ userAccess: { userId: ua.userId, allowed: !next.denied, tools: next.tools } })
                }}
                multiple
                size="sm"
                placeholder="All tools"
                class="w-full"
              />
            {/snippet}
          </McpAccessRow>
        {/each}
      </div>
    {/if}
    </div>
  </div>
  {#if error}<div transition:slide={{ duration: 150 }} class="mt-3 text-xs text-danger">{error}</div>{/if}
</Modal>
