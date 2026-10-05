<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { X } from '@lucide/svelte'
  import ChannelView from '@/components/chat/ChannelView.svelte'
  import ChannelComposer from '@/components/chat/ChannelComposer.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { splitAttachments, type Attachment } from '@/lib/attachments'
  import { openGroupDm, sendChannelMessage, type Channel } from '@/lib/channels.svelte'
  import { findDm, groupDmLabel, isGroupDm } from '@/lib/comms-dm'
  import { agentPresence } from '@/lib/comms-sidebar'
  import { toastError } from '@/lib/toast.svelte'
  import type { AgentModel } from '@/lib/agents'
  import type { AgentStatus } from '@/lib/fleet'
  import type { DirectoryUser } from '@/lib/users'
  import PresenceAvatar from './PresenceAvatar.svelte'

  // The "New message" pane (/comms/new). Pick any mix of people and agents in
  // the To: field — or a channel, which just opens it. With two or more
  // recipients a Name field appears (optional). If those recipients already
  // have a DM, its history shows below and the composer writes there; if not,
  // the first message creates the conversation (POST /api/dms) and it lands
  // under Direct messages, named or labelled with everyone's names.
  //
  // One agent on its own isn't a DM: it has its own conversation, so the pane
  // offers to open that instead.
  let {
    selfId,
    users,
    fleet,
    fleetStatus,
    channels,
    onOpenChannel,
    onOpenAgent,
  }: {
    selfId: string | null
    users: DirectoryUser[]
    fleet: AgentModel[]
    fleetStatus: Map<string, AgentStatus>
    channels: Channel[]
    onOpenChannel: (id: string) => void
    onOpenAgent: (model: string) => void
  } = $props()

  const qc = useQueryClient()

  type Recipient = { kind: 'person'; id: string; label: string } | { kind: 'agent'; id: string; label: string }
  type Suggestion = Recipient | { kind: 'channel'; id: string; label: string }

  let recipients = $state<Recipient[]>([])
  let query = $state('')
  let name = $state('')
  // Open from the start: the To: field is focused on arrival, and it shows
  // everyone before you type.
  let open = $state(true)
  let highlighted = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)

  const personLabel = (u: DirectoryUser) => u.name ?? u.email ?? 'teammate'
  const agentLabel = (model: string) => fleet.find((a) => a.id === model)?.label ?? model

  const userIds = $derived(recipients.filter((r) => r.kind === 'person').map((r) => r.id))
  const agentIds = $derived(recipients.filter((r) => r.kind === 'agent').map((r) => r.id))
  const loneAgent = $derived(userIds.length === 0 && agentIds.length === 1 ? agentIds[0] : null)
  const existing = $derived(findDm(channels, selfId, userIds, agentIds))
  // Name shows once it's a group; a found DM already has its own.
  const showName = $derived(recipients.length >= 2 && !existing)
  const recipientNames = $derived(
    recipients.map((r) => (r.kind === 'person' ? (r.label.split(' ')[0] ?? r.label) : r.label)).join(', '),
  )

  const suggestions: Suggestion[] = $derived.by(() => {
    const q = query.trim().replace(/^[#@]/, '').toLowerCase()
    const taken = new Set(recipients.map((r) => `${r.kind}:${r.id}`))
    const hit = (...fields: (string | null | undefined)[]) => !q || fields.some((f) => f?.toLowerCase().includes(q))
    const people: Suggestion[] = users
      .filter((u) => u.id !== selfId && !taken.has(`person:${u.id}`) && hit(u.name, u.email))
      .map((u) => ({ kind: 'person', id: u.id, label: personLabel(u) }))
    const agents: Suggestion[] = fleet
      .filter((a) => !taken.has(`agent:${a.id}`) && hit(a.label, a.role, a.id))
      .map((a) => ({ kind: 'agent', id: a.id, label: a.label }))
    // Channels only while nobody is picked yet — picking one opens it.
    const rooms: Suggestion[] =
      recipients.length > 0
        ? []
        : channels
            .filter((c) => c.kind !== 'dm' && hit(c.name))
            .map((c) => ({ kind: 'channel', id: c.id, label: c.name }))
    return [...people, ...agents, ...rooms].slice(0, 12)
  })

  $effect(() => {
    void suggestions.length
    highlighted = 0
  })

  const pick = (s: Suggestion) => {
    if (s.kind === 'channel') return onOpenChannel(s.id)
    recipients = [...recipients, s]
    query = ''
    inputEl?.focus()
  }
  const remove = (r: Recipient) => {
    recipients = recipients.filter((x) => !(x.kind === r.kind && x.id === r.id))
    inputEl?.focus()
  }

  const onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'ArrowDown' && suggestions.length) {
      e.preventDefault()
      open = true
      highlighted = (highlighted + 1) % suggestions.length
    } else if (e.key === 'ArrowUp' && suggestions.length) {
      e.preventDefault()
      highlighted = (highlighted - 1 + suggestions.length) % suggestions.length
    } else if (e.key === 'Enter') {
      const s = suggestions[highlighted]
      if (open && s) {
        e.preventDefault()
        pick(s)
      }
    } else if (e.key === 'Backspace' && !query && recipients.length) {
      recipients = recipients.slice(0, -1)
    } else if (e.key === 'Escape') {
      open = false
    }
  }

  // The first message creates the conversation, then the view moves to it.
  const send = async (text: string, atts: Attachment[]) => {
    if (recipients.length === 0) {
      toastError('Pick someone first', new Error('Add a person or an agent in To: before sending.'))
      return
    }
    try {
      const dm = await openGroupDm({ userIds, agents: agentIds, name: showName ? name : null })
      const { attachmentIds, refs } = splitAttachments(atts)
      await sendChannelMessage(dm.id, text, attachmentIds, refs)
      await qc.invalidateQueries({ queryKey: ['channels'] })
      onOpenChannel(dm.id)
    } catch (e) {
      toastError('Could not start the conversation', e)
    }
  }

  const hint = $derived(
    [
      'It shows up under Direct messages once you send.',
      agentIds.length > 1
        ? '@mention an agent to have it reply.'
        : agentIds.length === 1
          ? `${agentLabel(agentIds[0] ?? '')} replies to every message.`
          : '',
    ]
      .filter(Boolean)
      .join(' '),
  )

  const existingLabel = $derived(
    existing
      ? isGroupDm(existing)
        ? groupDmLabel(existing, agentLabel)
        : (existing.peer?.name ?? existing.peer?.email ?? 'teammate')
      : '',
  )
</script>

<div class="flex h-full min-h-0 flex-col">
  <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
    <span class="text-sm font-semibold text-fg">New message</span>
  </header>

  <!-- To: — chips for who's picked, then the search that adds the next one. -->
  <div class="relative shrink-0 border-b border-line-subtle px-5 py-2">
    <div class="flex flex-wrap items-center gap-1.5">
      <span class="mr-1 font-sans text-sm text-muted">To:</span>
      {#each recipients as r (`${r.kind}:${r.id}`)}
        {@const u = r.kind === 'person' ? users.find((x) => x.id === r.id) : undefined}
        <span class="flex items-center gap-1.5 rounded-md bg-accent-soft py-0.5 pl-1 pr-1.5 font-sans text-sm text-fg">
          {#if r.kind === 'person'}
            <PresenceAvatar name={r.label} src={u?.picture} presence={u?.online ? 'online' : 'offline'} cutout="ring-surface" />
          {:else}
            <PresenceAvatar name={r.label} presence={agentPresence(fleetStatus.get(r.id))} cutout="ring-surface" />
          {/if}
          {r.label}
          <button
            type="button"
            class="grid h-4 w-4 place-items-center rounded text-muted hover:text-fg"
            title={`Remove ${r.label}`}
            aria-label={`Remove ${r.label}`}
            onclick={() => remove(r)}
          >
            <X size={12} />
          </button>
        </span>
      {/each}
      <!-- svelte-ignore a11y_autofocus -- reason: this pane exists to type a recipient; focusing it is the point of opening it -->
      <input
        bind:this={inputEl}
        bind:value={query}
        autofocus
        class="min-w-40 flex-1 bg-transparent py-1 font-sans text-sm text-fg outline-none placeholder:text-ink-dim"
        placeholder={recipients.length ? '' : 'A person, an agent, or #a-channel'}
        aria-label="To"
        role="combobox"
        aria-expanded={open && suggestions.length > 0}
        aria-controls="new-message-suggestions"
        onfocus={() => (open = true)}
        oninput={() => (open = true)}
        onblur={() => setTimeout(() => (open = false), 120)}
        onkeydown={onKeydown}
      />
    </div>
    {#if open && suggestions.length > 0}
      <ul
        id="new-message-suggestions"
        role="listbox"
        class="absolute inset-x-4 top-full z-20 mt-1 max-h-80 overflow-y-auto rounded-lg border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]"
      >
        {#each suggestions as s, i (`${s.kind}:${s.id}`)}
          {@const u = s.kind === 'person' ? users.find((x) => x.id === s.id) : undefined}
          {@const a = s.kind === 'agent' ? fleet.find((x) => x.id === s.id) : undefined}
          <li role="option" aria-selected={i === highlighted}>
            <button
              type="button"
              class={cn(
                'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left font-sans text-sm text-fg',
                i === highlighted ? 'bg-card2' : 'hover:bg-card2',
              )}
              onmouseenter={() => (highlighted = i)}
              onmousedown={(e) => e.preventDefault()}
              onclick={() => pick(s)}
            >
              {#if s.kind === 'person'}
                <PresenceAvatar name={s.label} src={u?.picture} presence={u?.online ? 'online' : 'offline'} cutout="ring-panel" />
                <span class="font-medium">{s.label}</span>
                {#if u?.title}<span class="truncate text-xs text-muted">{u.title}</span>{/if}
              {:else if s.kind === 'agent'}
                <PresenceAvatar name={s.label} presence={agentPresence(fleetStatus.get(s.id))} cutout="ring-panel" />
                <span class="font-medium">{s.label}</span>
                <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">agent</span>
                {#if a?.role}<span class="truncate text-xs text-muted">{a.role}</span>{/if}
              {:else}
                <span class="grid h-5 w-5 place-items-center text-muted">#</span>
                <span class="font-medium">{s.label}</span>
              {/if}
              {#if i === highlighted}<span class="ml-auto font-mono text-[10px] text-ink-dim">Enter</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  {#if showName}
    <div class="flex shrink-0 items-center gap-2 border-b border-line-subtle px-5 py-2">
      <span class="font-sans text-sm text-muted">Name:</span>
      <input
        bind:value={name}
        maxlength={80}
        class="flex-1 bg-transparent py-1 font-sans text-sm text-fg outline-none placeholder:text-ink-dim"
        placeholder="Name the conversation (optional)"
        aria-label="Conversation name"
      />
    </div>
  {/if}

  <div class="min-h-0 flex-1">
    {#if existing}
      <!-- These recipients already talk here: show it, and write there. -->
      {#key existing.id}
        <ChannelView
          channelId={existing.id}
          channelName={existingLabel}
          {fleet}
          composerPlaceholder={`Message ${isGroupDm(existing) ? existingLabel : (existingLabel.split(' ')[0] ?? existingLabel)}`}
        />
      {/key}
    {:else if loneAgent}
      <div class="grid h-full place-items-center p-6">
        <div class="max-w-sm text-center">
          <p class="font-sans text-sm text-fg">{agentLabel(loneAgent)} has its own conversation.</p>
          <p class="mt-1 font-sans text-xs text-muted">Add a person or another agent to start a group DM.</p>
          <Button class="mt-3" size="sm" onclick={() => onOpenAgent(loneAgent)}>Open {agentLabel(loneAgent)}</Button>
        </div>
      </div>
    {:else}
      <div class="flex h-full flex-col">
        <div class="grid flex-1 place-items-center p-6 text-center">
          {#if recipients.length === 0}
            <p class="font-sans text-sm text-muted">Pick people and agents above to start a conversation.</p>
          {:else}
            <div>
              <p class="font-sans text-sm text-fg">This is the start of your conversation with {recipientNames}.</p>
              <p class="mt-1 font-sans text-xs text-muted">{hint}</p>
            </div>
          {/if}
        </div>
        <div class="mx-auto w-full max-w-[var(--converse-width)]">
          <ChannelComposer
            channelName="new"
            placeholder={recipients.length ? `Message ${recipientNames}` : 'Start a new message'}
            mentionables={[]}
            onSend={send}
          />
        </div>
      </div>
    {/if}
  </div>
</div>
