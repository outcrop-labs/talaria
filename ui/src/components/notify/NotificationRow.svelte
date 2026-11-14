<script lang="ts">
  import { AtSign, Bell, Bot, Check, CircleAlert, Eye, Mail, MailOpen, Server, Undo2 } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { countLabel, reasonOf, type CenterRow } from '@/lib/notify-center'
  import type { ClassInfo } from '@/lib/notify-wire'

  // ONE NOTIFICATION, in the bell and on /notifications: who (the actor's
  // avatar, or the agent glyph), what (title, body), why (the registry's
  // reason for its kind), how many (a coalesced row's count) and when. The
  // whole text is the open target; the actions beside it are their own
  // buttons, never nested in it.
  //
  // The read state is the dot and the weight: accent dot and full weight while
  // it waits, muted once read — the eye finds what is NEW, not what is merely
  // recent (wave 1's bell, kept).
  let {
    row,
    classes,
    picture = null,
    compact = false,
    focused = false,
    selected = false,
    selectable = false,
    canArchive = false,
    canUnread = false,
    onOpen,
    onDone,
    onRestore,
    onToggleRead,
    onSelect,
  }: {
    row: CenterRow
    classes: readonly ClassInfo[] | undefined
    /** The actor's photo, from the people directory when it is cached. */
    picture?: string | null
    compact?: boolean
    focused?: boolean
    selected?: boolean
    selectable?: boolean
    /** Done/Move to inbox exist (`inbox-v2`). */
    canArchive?: boolean
    /** Mark unread exists (`inbox-v2`). */
    canUnread?: boolean
    onOpen: (row: CenterRow) => void
    onDone?: (row: CenterRow) => void
    onRestore?: (row: CenterRow) => void
    onToggleRead?: (row: CenterRow) => void
    onSelect?: (row: CenterRow) => void
  } = $props()

  const unread = $derived(!row.readAt)
  const archived = $derived(!!row.archivedAt)
  const reason = $derived(reasonOf(row, classes))
  const count = $derived(countLabel(row))
  const actor = $derived(row.actor ?? null)
  const meta = $derived([reason, count, relativeTime(row.createdAt)].filter(Boolean).join(' · '))
  // A row with no person or agent behind it shows what kind of thing it is,
  // so every title starts at the same place (Linear/GitHub style).
  const CategoryIcon = $derived(
    row.category === 'needs_you'
      ? CircleAlert
      : row.category === 'direct'
        ? AtSign
        : row.category === 'following'
          ? Eye
          : row.category === 'workspace'
            ? Server
            : Bell,
  )
</script>

<div
  data-notification-row={row.id}
  data-unread={unread ? 'true' : 'false'}
  data-focused={focused ? 'true' : undefined}
  data-selected={selected ? 'true' : undefined}
  class={cn(
    'group/nrow relative flex items-start gap-2.5 border-b border-line px-3 transition-colors duration-[120ms] last:border-b-0',
    // The page's rows breathe a little more than the bell's.
    compact ? 'py-2' : 'py-2.5',
    focused && 'bg-raised',
    selected && 'bg-accent-soft',
  )}
>
  {#if focused}
    <span aria-hidden="true" class="absolute inset-y-1 left-0 w-0.5 rounded-full bg-accent"></span>
  {/if}
  {#if selectable}
    <Checkbox bare checked={selected} title={selected ? 'Unselect' : 'Select'} onChange={() => onSelect?.(row)} class="mt-[7px]" />
  {/if}
  {#if unread}
    <StatusDot status="accent" class="mt-[9px]" />
  {:else}
    <span class="mt-[9px] h-1.5 w-1.5 shrink-0"></span>
  {/if}
  {#if actor?.kind === 'agent'}
    <span
      class="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full border border-line-strong bg-raised text-muted"
      title={actor.name ?? 'Agent'}
    >
      <Bot size={13} strokeWidth={1.5} />
    </span>
  {:else if actor?.kind === 'user'}
    <Avatar src={picture} name={actor.name} class="mt-0.5 h-6 w-6" />
  {:else}
    <span class="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full text-ink-dim" aria-hidden="true" data-row-glyph={row.category ?? 'none'}><!-- voice-ok: the bell and the inbox page, outside the settings area -->
      <CategoryIcon size={13} strokeWidth={1.5} />
    </span>
  {/if}
  <button
    type="button"
    onclick={() => onOpen(row)}
    class="min-w-0 flex-1 text-left"
    title={row.title}
  >
    <span class="block truncate text-sm {unread ? 'font-medium text-fg' : 'text-muted'}">{row.title}</span>
    {#if row.body}
      <span class="mt-0.5 block truncate text-xs {unread ? 'text-muted' : 'text-ink-dim'}">{row.body}</span><!-- voice-ok: the bell and the inbox page, outside the settings area -->
    {/if}
    <span class="mt-0.5 block truncate font-mono text-[10px] text-ink-dim" data-row-meta>{meta}</span><!-- voice-ok: the bell and the inbox page, outside the settings area -->
  </button>
  <div
    class={cn(
      'flex shrink-0 items-center gap-0.5 transition-opacity',
      // No hover on a touch screen: the actions stay visible there.
      focused || selected
        ? 'opacity-100'
        : 'opacity-0 focus-within:opacity-100 group-hover/nrow:opacity-100 [@media(hover:none)]:opacity-100',
    )}
  >
    {#if canArchive && archived && onRestore}
      <IconButton size="sm" title="Move to inbox" onclick={() => onRestore(row)}>
        <Undo2 size={14} strokeWidth={1.5} />
      </IconButton>
    {:else if canArchive && onDone}
      <IconButton size="sm" title="Done" onclick={() => onDone(row)}>
        <Check size={14} strokeWidth={1.5} />
      </IconButton>
    {/if}
    {#if onToggleRead && (unread || canUnread) && !archived}
      <IconButton size="sm" title={unread ? 'Mark read' : 'Mark unread'} onclick={() => onToggleRead(row)}>
        {#if unread}<MailOpen size={14} strokeWidth={1.5} />{:else}<Mail size={14} strokeWidth={1.5} />{/if}
      </IconButton>
    {/if}
  </div>
</div>
