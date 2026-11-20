<script lang="ts">
  // WHO OWNS THIS AGENT. Managers are the only people — admins aside — who
  // can change it: its identity, soul, skills, memory, crons, secrets, MCP
  // binds and lifecycle. Handing it over is itself a change to the agent, so
  // this list is manager-gated like everything else here.
  //
  // THE LIST IS NEVER EMPTY, and the last row's remove button says so rather
  // than failing after the click: a manager who cleared the roster would lock
  // themselves out of their own agent and need an admin to get back in. To
  // hand the agent to somebody else, add them first, then drop yourself.
  //
  // Admins are not listed as managers unless they were named — they hold
  // reach by role, which the footer says once instead of pretending every
  // admin is on the roster.
  import { createRawSnippet, mount, unmount, type Component, type Snippet } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ComboOption } from '@/components/ui/combobox'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { errorMessage } from '@/lib/fetch-json'
  import { setAgentManagers, useAgentManagers, type AgentDef, type AgentManager } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import { useUsers } from '@/lib/users'

  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  const managersQuery = useAgentManagers(() => def.id)
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })

  let busy = $state(false)
  let msg = $state<string | null>(null)

  const labelOf = (m: AgentManager) => m.name ?? m.email ?? m.userId

  // Same trick the share dialog uses: ComboOption.icon is a zero-arg Snippet,
  // and these options are built in script from directory data.
  const componentIcon = (component: Component<any>, props: Record<string, unknown>, wrapperClass = 'contents'): Snippet =>
    createRawSnippet(() => ({
      render: () => `<span class="${wrapperClass}"></span>`,
      setup(el) {
        const instance = mount(component, { target: el, props })
        return () => unmount(instance)
      },
    }))

  const addOptions = $derived.by((): ComboOption[] => {
    const named = new Set((managersQuery.data ?? []).map((m) => m.userId))
    return usersList.rows
      .filter((u) => !named.has(u.id))
      .map((u) => ({
        value: u.id,
        label: u.name ?? u.email ?? u.id,
        sub: 'Can change this agent',
        icon: componentIcon(Avatar, { name: u.name ?? u.email, class: 'h-5 w-5' }),
      }))
  })

  // One PUT per change, over the roster the modal is showing. A failed write
  // leaves the list exactly as the server has it — the refetch below is what
  // the rows render from, never an optimistic guess about who may change an
  // agent.
  const write = async (next: string[]) => {
    busy = true
    msg = null
    try {
      await setAgentManagers(def.id, next)
      await qc.invalidateQueries({ queryKey: ['agent-managers', def.id] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } catch (e) {
      msg = errorMessage(e)
    } finally {
      busy = false
    }
  }

  const add = (id: string) => {
    if (!id) return
    void write([...(managersQuery.data ?? []).map((m) => m.userId), id])
  }
  const remove = (id: string) => void write((managersQuery.data ?? []).filter((m) => m.userId !== id).map((m) => m.userId))
</script>

<div class="space-y-4">
  <div class="flex items-center gap-4">
    <p class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
      Managers can change this agent's settings, skills, memory, schedules and secrets.
    </p>
    {#if canManage}
      <Combobox
        options={addOptions}
        selected={[]}
        size="sm"
        placeholder="Add a manager"
        disabled={busy || managersQuery.data === undefined}
        onChange={(v) => add(v[0] ?? '')}
        class="w-[280px] shrink-0"
      />
    {/if}
  </div>

  {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
  {#if msg}<QueryError variant="inline" title="Could not change the managers" error={msg} />{/if}

  <QueryState query={managersQuery} errorTitle="Could not load this agent's managers" errorVariant="compact" isEmpty={(m) => m.length === 0}>
    {#snippet skeleton()}
      <SettingsGroup aria-hidden="true">
        {#each [0, 1] as i (i)}
          <div class="flex min-h-[60px] items-center gap-3 px-5 py-4">
            <Skeleton class="h-8 w-8 rounded-full" />
            <Skeleton class="h-3.5 w-40 rounded-full" />
          </div>
        {/each}
      </SettingsGroup>
    {/snippet}
    {#snippet empty()}
      <!-- Reachable only for an agent whose managers all left the org: the
           PUT refuses an empty set, so nobody can arrive here by clicking. -->
      <SettingsGroup>
        <p class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">
          Nobody manages this agent. Admins can still change it, and naming a manager here hands it over.
        </p>
      </SettingsGroup>
    {/snippet}
    {#snippet children(managers)}
      <div use:listStagger>
        <SettingsGroup>
          {#each managers as m (m.userId)}
            <SettingsRow label={labelOf(m)} hint={m.name && m.email ? m.email : undefined}>
              {#snippet lead()}<Avatar name={labelOf(m)} class="h-8 w-8" />{/snippet}
              {#snippet meta()}{#if m.role === 'admin'}<Chip>Admin</Chip>{/if}{/snippet}
              {#if canManage}
                <span title={managers.length === 1 ? 'An agent keeps at least one manager. Add someone else first.' : undefined}>
                  <DangerLink onClick={() => remove(m.userId)} disabled={busy || managers.length === 1}>Remove</DangerLink>
                </span>
              {/if}
            </SettingsRow>
          {/each}
        </SettingsGroup>
      </div>
    {/snippet}
  </QueryState>

  <p class="font-sans text-[13px] leading-5 text-muted">Admins can change any agent, whether or not they are named here.</p>
</div>
