<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import {
    addChannelAgent,
    addChannelMember,
    deleteChannel,
    removeChannelAgent,
    removeChannelMember,
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

  const onDelete = async () => {
    if (!(await confirm({ title: 'Delete channel', message: `Delete #${channelName} and all its messages?`, confirmLabel: 'Delete', danger: true }))) return
    void run(() => deleteChannel(channelId)).then(onDeleted)
  }
</script>

<Modal {open} {onClose} title={`#${channelName} settings`}>
  <div class="space-y-5">
    <section>
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People</div>
      <ul class="space-y-1">
        {#each detail.members as m (m.userId)}
          <li class="flex items-center gap-2 text-sm">
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
      <div class="text-sm" style="color: var(--theme-danger)">
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
