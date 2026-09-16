<script lang="ts">
  import { useQueryClient, createQuery } from '@tanstack/svelte-query'
  import { X } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { fade, listStagger, slide } from '@/lib/motion'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { getJson } from '@/lib/fetch-json'
  import {
    addChannelAgent,
    addChannelMember,
    addChannelTeam,
    deleteChannel,
    removeChannelAgent,
    removeChannelMember,
    removeChannelTeam,
    type ChannelDetail,
  } from '@/lib/channels.svelte'
  import type { AgentModel } from '@/lib/agents'
  import type { DirectoryUser } from '@/lib/users'

  // Channel settings: people + agents, and the owner's delete. One modal,
  // mirroring Board settings' People/Agents structure.
  let {
    open,
    onClose,
    channelId,
    channelName,
    detail,
    fleet,
    selfUserId,
    onDeleted,
  }: {
    open: boolean
    onClose: () => void
    channelId: string
    channelName: string
    detail: ChannelDetail
    fleet: AgentModel[]
    selfUserId: string | null
    onDeleted: () => void
  } = $props()

  const qc = useQueryClient()
  let error = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['channel', channelId] })
  const dirQuery = createQuery(() => ({
    queryKey: ['teams-directory'],
    enabled: open,
    queryFn: () => getJson<{ teams: Array<{ id: string; name: string }> }>('/api/teams/directory'),
  }))
  const mineQuery = createQuery(() => ({
    queryKey: ['teams'],
    enabled: open,
    queryFn: () => getJson<{ teams: Array<{ id: string }> }>('/api/teams'),
  }))
  const teams = $derived(detail.teams ?? [])
  const mine = $derived(new Set((mineQuery.data?.teams ?? []).map((t) => t.id)))
  const teamOptions = $derived(
    (dirQuery.data?.teams ?? [])
      .filter((t) => !teams.some((g) => g.id === t.id))
      .map((t) => ({ value: t.id, label: t.name })),
  )

  const run = async (fn: () => Promise<void>) => {
    error = null
    try {
      await fn()
      await refresh()
    } catch (e) {
      error = (e as Error).message
    }
  }

  const isOwner = $derived(detail.role === 'owner')
  const agentOptions = $derived(fleet.map((a) => ({ value: a.id, label: a.label, sub: a.role })))

  // The combobox toggles one agent per change — diff against the channel's
  // current set and apply immediately (membership is instant, like People).
  const setAgents = (next: string[]) => {
    const cur = new Set(detail.agents)
    const nextSet = new Set(next)
    const added = next.find((m) => !cur.has(m))
    const removed = detail.agents.find((m) => !nextSet.has(m))
    if (added) void run(() => addChannelAgent(channelId, added))
    if (removed) void run(() => removeChannelAgent(channelId, removed))
  }

  const pickUser = (u: DirectoryUser) => {
    const email = u.email
    if (email) void run(() => addChannelMember(channelId, email))
  }
  const pickTeam = (next: string[]) => {
    const id = next[0]
    if (id) void run(() => addChannelTeam(channelId, id))
  }

  const onDelete = async () => {
    if (!(await confirm({ title: 'Delete channel', message: `Delete #${channelName} and all its messages?`, confirmLabel: 'Delete', danger: true }))) return
    void run(() => deleteChannel(channelId)).then(onDeleted)
  }
</script>

<Modal {open} {onClose} title={`#${channelName} settings`}>
  <div class="space-y-5">
    <section>
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People</div>
      <ul class="space-y-1" use:listStagger>
        {#each detail.members as m (m.userId)}
          <!-- Fires only for live add/remove while the modal is open — the
              Modal's own transition covers the initial roster (local default). -->
          <li in:fade={{ duration: 150 }} out:slide={{ duration: 150 }} class="flex items-center gap-2 text-sm">
            <Avatar name={m.name ?? m.email} class="h-6 w-6 text-xs" />
            <span class="min-w-0 flex-1 truncate">
              {m.name ?? m.email}
              {#if m.name && m.email}<span class="ml-1.5 text-xs text-muted">{m.email}</span>{/if}
            </span>
            <span class="text-xs text-muted">{m.role}</span>
            {#if m.role !== 'owner' && (isOwner || m.userId === selfUserId)}
              <Button
                variant="ghost"
                size="sm"
                onclick={() => void run(() => removeChannelMember(channelId, m.userId))}
              >
                {m.userId === selfUserId ? 'Leave' : 'Remove'}
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
      <UserPicker class="mt-2" exclude={detail.members.map((m) => m.userId)} onPick={pickUser} />
    </section>

    <section>
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Teams</div>
      <ul class="mb-2 flex flex-wrap gap-1.5">
        {#each teams as t (t.id)}
          <li class="flex items-center gap-1 rounded-full border border-line bg-raised px-2 py-0.5 text-xs">
            <span class="max-w-40 truncate">{t.name}</span>
            {#if isOwner || mine.has(t.id)}
              <button
                type="button"
                title={`Remove ${t.name}`}
                onclick={() => void run(() => removeChannelTeam(channelId, t.id))}
                class="grid h-3.5 w-3.5 place-items-center rounded-full text-muted hover:text-fg"
              >
                <X size={9} />
              </button>
            {/if}
          </li>
        {/each}
      </ul>
      <Combobox options={teamOptions} selected={[]} onChange={pickTeam} placeholder="Add a team" />
    </section>

    <section>
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agents</div>
      <p class="mb-2 text-xs text-muted">@mention an agent in the channel to bring it into the conversation.</p>
      <Combobox
        options={agentOptions}
        selected={detail.agents}
        onChange={setAgents}
        multiple
        placeholder="Select agents"
      />
    </section>

    {#if error}
      <div transition:slide={{ duration: 150 }} class="text-sm" style="color: var(--theme-danger)">
        {error}
      </div>
    {/if}

    {#if isOwner}
      <section class="flex justify-end border-t border-line-subtle pt-3">
        <DangerLink onClick={onDelete}>Delete channel</DangerLink>
      </section>
    {/if}
  </div>
</Modal>
