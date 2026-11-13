<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { cardLine, panelHref, serverCalls, useMcpCallsSummary } from '@/components/observability/mcp-calls'
  import { cn } from '@/lib/cn'
  import { useAgents } from '@/lib/agents'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'
  import { useUsers } from '@/lib/users'
  import McpAccessRow from './McpAccessRow.svelte'
  import McpAddPickerButton from './McpAddPickerButton.svelte'
  import {
    ALL_TOOLS,
    NO_ACCESS,
    STATUS_DOT,
    checkedLine,
    healthChecked,
    patchServer,
    resolveScopePick,
    serverDomain,
    serverStatus,
    staleAssignments,
    useTeamsDirectory,
    type McpServerRow,
  } from './mcp'

  // ONE SERVER, ONE LEVEL DOWN — the dialog a row's Manage opens. The list
  // says what a server is and whether it works; everything you only need once
  // you are looking at one server lives here: what the last check said, its
  // tools, the day's calls, and WHO MAY USE IT. Access is a table that grows
  // with the org (a row per agent, per person, per team, each with a tool
  // subset), which is why it never sat open in the list.
  let {
    server: s,
    onClose,
    onRemove,
  }: {
    server: McpServerRow
    onClose: () => void
    /** Remove the server, for one that can be removed. The row owns the confirmation. */
    onRemove?: () => void
  } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  // Access RULES are shown per person by name. Defaulted to `[]` a failed
  // directory read rendered each existing rule as a truncated raw id and the
  // "add a person" picker as empty — an access table that looks like it is
  // about strangers, on a server that grants tool use.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const users = $derived(usersList.rows)
  const teamsList = listQuery(useTeamsDirectory(), { title: 'Could not load teams', variant: 'inline' })
  const teams = $derived(teamsList.rows)
  let error = $state<string | null>(null)
  let asking = $state(false)

  const patch = async (body: unknown) => {
    error = null
    const e = await patchServer(s.id, body)
    if (e) error = e
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
  }
  // The handshake, the tool list, and whether the credentials still work: one
  // request, whether it is asked as "check now" or "refresh tools".
  const askServer = () => {
    asking = true
    void patch({ refreshTools: true }).finally(() => (asking = false))
  }

  const status = $derived(serverStatus(s))
  const domain = $derived(serverDomain(s.url))
  // Only servers the hourly health pass asks have a "last check": the
  // deployment's own and stdio packages are not probed, so a failure they
  // report carries its own timestamp instead.
  const probed = $derived(healthChecked(s))
  const checked = $derived(probed ? checkedLine(s.probe, relativeTime) : null)
  // The Workbench is granted by each agent's Developer Agent switch, not from
  // here. The api refuses assignment and access rows for it.
  const isWorkbench = $derived(s.name === 'workbench')
  const stale = $derived(staleAssignments(s.assignments, s.tools))
  // The last 24 hours at the gateway, opening the full record filtered to
  // this server. Admins only, because the record is.
  const session = useSession()
  const callsDay = useMcpCallsSummary('24h', () => session.data?.role === 'admin')
  const callsLine = $derived(callsDay.data ? cardLine(serverCalls(callsDay.data, s.id), 'Last 24 hours') : null)

  const agentOptions = $derived((fleetQuery.data?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))
  const agentLabel = (model: string) => agentOptions.find((o) => o.value === model)?.label ?? model
  const toolOptions = $derived(s.tools.map((t) => ({ value: t.name, label: t.name })))
  const userLabel = (id: string) => {
    const u = users.find((x) => x.id === id)
    return u?.name ?? u?.email ?? id.slice(0, 8)
  }
  const teamLabel = (id: string) => teams.find((t) => t.id === id)?.name ?? id.slice(0, 8)

  const fact = 'min-w-0 font-sans text-[13px] leading-5 text-fg'
  const quiet = 'font-sans text-[13px] leading-5 text-muted'
</script>

{#snippet heading(title: string, info?: string)}
  <h3 class="font-sans text-[13px] font-medium leading-5 text-fg">{title}</h3>
  {#if info}<InfoTip text={info} />{/if}
{/snippet}

<Modal open {onClose} title={s.label} width="max-w-2xl">
  <div class="space-y-6">
    <dl class="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-4 gap-y-2">
      <dt class={quiet}>Status</dt>
      <dd class={fact}>
        <span class="flex items-center gap-2"><StatusDot status={STATUS_DOT[status.tone]} />{status.label}</span>
        <!-- The server's own words, so a 401 reads as the server's 401. -->
        {#if status.kind === 'app-off'}
          <p class={cn('mt-1', quiet)}>Off because {s.appOff?.replace(/\.$/, '')}. No agent can use it until the app is back, and every access rule is kept.</p>
        {:else if status.detail && status.tone !== 'success'}
          <p class={cn('mt-1', status.tone === 'danger' ? 'font-sans text-[13px] leading-5 text-danger' : quiet)}>
            {status.detail}
            {#if !probed && s.probe?.at}<span class="text-muted" title={new Date(s.probe.at).toLocaleString()}>· {relativeTime(s.probe.at)}</span>{/if}
          </p>
        {/if}
      </dd>
      {#if domain}
        <dt class={quiet}>Address</dt>
        <dd class={cn(fact, 'truncate font-mono text-xs text-muted')}>{domain}</dd>
      {/if}
      {#if probed && s.enabled}
        <dt class={quiet}>Last check</dt>
        <dd class={cn(fact, 'flex items-center gap-3')}>
          <span title={s.probe?.at ? new Date(s.probe.at).toLocaleString() : undefined}>{checked ?? 'Not checked yet'}</span>
          <Button
            variant="link"
            size="xs"
            class="gap-1.5"
            onclick={askServer}
            disabled={asking}
            title="Ask the server again now: the handshake, its tool list, and whether the credentials still work"
          >
            {#if asking}<WaitingMark site="mcp/server-refresh" size={12} />{/if}
            Check now
          </Button>
        </dd>
      {/if}
      {#if callsLine}
        <dt class={quiet}>Calls</dt>
        <dd class={fact}>
          <a href={panelHref(s.name)} class="text-fg underline decoration-line underline-offset-4 transition-colors hover:decoration-fg" title="Open this server's calls in Observability">
            {callsLine}
          </a>
        </dd>
      {/if}
    </dl>

    <section>
      <div class="mb-2 flex items-center gap-2">
        {@render heading('Tools')}
        <span class={cn(quiet, 'tabular-nums')}>
          {s.toolsRefreshedAt ? `${s.tools.length} · updated ${relativeTime(s.toolsRefreshedAt)}` : s.tools.length}
        </span>
        <span class="flex-1"></span>
        {#if !(probed && s.enabled)}
          <Button variant="ghost" size="xs" class="gap-1.5" onclick={askServer} disabled={asking} title="Ask the server for its tool catalog">
            {#if asking}<WaitingMark site="mcp/server-refresh" size={12} />{/if}
            {s.tools.length ? 'Refresh' : 'Discover tools'}
          </Button>
        {/if}
      </div>
      {#if s.tools.length}
        <div class="flex max-h-[132px] flex-wrap gap-1.5 overflow-y-auto rounded-md border border-line-subtle p-2.5">
          {#each s.tools as t (t.name)}
            <Chip title={t.description} class="font-mono font-normal">{t.name}</Chip>
          {/each}
        </div>
      {:else}
        <p class={quiet}>No tools found yet.</p>
      {/if}
      {#if stale.length && s.enabled}
        <p class="mt-2 font-sans text-[13px] leading-5 text-warning" title={stale.map((a) => `${a.agentModel}: ${a.missing.join(', ')}`).join('\n')}>
          {stale.length} agent{stale.length === 1 ? '' : 's'} name{stale.length === 1 ? 's' : ''} tools this server no longer offers
          ({[...new Set(stale.flatMap((a) => a.missing))].join(', ')}). Drop them on each agent's MCP tab.
        </p>
      {/if}
    </section>

    {#if isWorkbench}
      <section class="border-t border-line-subtle pt-5">
        <div class="mb-2 flex items-center gap-2">{@render heading('Agents')}</div>
        <p class={quiet}>Every Developer Agent. Turn it on in an agent's settings.</p>
      </section>
    {:else}
      <section class="border-t border-line-subtle pt-5">
        <div class="mb-1 flex items-center gap-2">
          {@render heading('Agents', 'Agents that can call this server. Leave an agent on all tools, or limit it to some.')}
          <span class="flex-1"></span>
          <McpAddPickerButton
            label="Add agent"
            title={s.allAgents ? 'Limit one agent to some tools' : 'Add an agent'}
            placeholder="Search agents"
            options={agentOptions.filter((o) => !s.assignments.some((a) => a.agentModel === o.value))}
            onPick={(m) => void patch({ assign: { agentModel: m, tools: null } })}
          />
        </div>
        {#if !s.builtin}
          <div class="flex items-center gap-3 py-1.5">
            <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-fg">Every agent</span>
            <Toggle checked={s.allAgents} onChange={(checked) => void patch({ allAgents: checked })} title="Every enabled agent can use this server" />
          </div>
        {/if}
        {#each s.assignments as a (a.agentModel)}
          <McpAccessRow name={agentLabel(a.agentModel)} onRemove={() => void patch({ unassign: a.agentModel })} removeTitle="Remove this agent">
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
        <p class={cn('py-1.5', quiet)}>
          {#if s.builtin}
            Every agent can use this. Add an agent to limit which tools it gets.
          {:else if s.allAgents}
            {s.assignments.length ? 'The agents listed are limited to the tools shown.' : 'Every agent gets every tool. Add an agent to limit it.'}
          {:else if s.assignments.length === 0}
            No agents yet. Add one, or turn on Every agent.
          {:else}
            Only the agents listed can use it.
          {/if}
        </p>
      </section>

      <section>
        <div class="mb-1 flex items-center gap-2">
          {@render heading('People', 'With no rules, everyone with an assigned agent may use it. A rule limits one person to some tools, or blocks them.')}
          <span class="flex-1"></span>
          <McpAddPickerButton
            label="Add person"
            title="Add a rule for one person"
            placeholder="Search people"
            options={users
              .filter((u) => !s.userAccess.some((r) => r.userId === u.id))
              .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id.slice(0, 8), sub: u.email ?? undefined }))}
            onPick={(id) => void patch({ userAccess: { userId: id, allowed: true, tools: null } })}
          />
        </div>
        {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
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
        {:else}
          <p class={cn('py-1.5', quiet)}>Everyone with an assigned agent may use it.</p>
        {/each}
      </section>

      <section>
        <div class="mb-1 flex items-center gap-2">
          {@render heading('Teams', "A team rule allows or blocks the team's people and agents. Limit it to some tools if needed.")}
          <span class="flex-1"></span>
          <McpAddPickerButton
            label="Add team"
            title="Add a rule for one team"
            placeholder="Search teams"
            options={teams.filter((t) => !s.teamAccess.some((r) => r.teamId === t.id)).map((t) => ({ value: t.id, label: t.name }))}
            onPick={(id) => void patch({ teamAccess: { teamId: id, allowed: true, tools: null } })}
          />
        </div>
        {#if teamsList.notice}<QueryError {...teamsList.notice} />{/if}
        {#each s.teamAccess as ta (ta.teamId)}
          <McpAccessRow
            name={teamLabel(ta.teamId)}
            dim={!ta.allowed}
            onRemove={() => void patch({ teamAccess: { teamId: ta.teamId, allowed: null, tools: null } })}
            removeTitle="Remove this rule (back to default access)"
          >
            {#snippet tools()}
              <Combobox
                options={[
                  { value: ALL_TOOLS, label: 'All tools' },
                  { value: NO_ACCESS, label: 'No access' },
                  ...toolOptions,
                ]}
                selected={ta.allowed ? (ta.tools ?? [ALL_TOOLS]) : [NO_ACCESS]}
                onChange={(sel) => {
                  const next = resolveScopePick(sel, { denied: !ta.allowed, tools: ta.tools })
                  void patch({ teamAccess: { teamId: ta.teamId, allowed: !next.denied, tools: next.tools } })
                }}
                multiple
                size="sm"
                placeholder="All tools"
                class="w-full"
              />
            {/snippet}
          </McpAccessRow>
        {:else}
          <p class={cn('py-1.5', quiet)}>No team rules.</p>
        {/each}
      </section>
    {/if}
    {#if error}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex items-center gap-2">
      {#if onRemove}<DangerLink onClick={onRemove}>Remove server</DangerLink>{/if}
      <span class="flex-1"></span>
      <Button size="sm" variant="outline" onclick={onClose}>Done</Button>
    </div>
  {/snippet}
</Modal>
