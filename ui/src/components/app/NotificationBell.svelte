<script lang="ts">
  import { Bell } from '@lucide/svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { useMarkNotificationsRead, useNotifications } from '@/lib/notifications'
  import { navigateHref } from '@/router'

  // The strip's bell: the notification inbox as a dropdown beside the account
  // chip. Clicking a row is "take me there AND clear it" in one motion — the
  // mark-read that this bell finally wires to a real click is what lets an
  // unread row ever leave the count, the brief, and (once push lands) the OS.
  //
  // The count follows the rail's `/home` doctrine: a failed read is `!`, never
  // a silent 0 — the bell is the one surface a person glances at to ask "is
  // anything waiting", and an outage must not answer "no".
  const query = useNotifications()
  const markRead = useMarkNotificationsRead()

  const unread: number | null = $derived(
    query.isError && query.data === undefined ? null : (query.data?.unread ?? 0),
  )
  const unreadLabel = $derived(unread === null ? '!' : unread > 99 ? '99+' : String(unread))
  const unreadTitle = $derived(
    unread === null ? 'Could not load your notifications' : undefined,
  )
  const rows = $derived(query.data?.notifications ?? [])

  // A row click clears its row and goes where it points, in that order — the
  // navigation must not wait on the PUT, and a notification with no href
  // (board-access outcomes) still deserves its click-to-clear.
  function open(n: { id: string; href: string }, close: () => void) {
    close()
    void markRead({ ids: [n.id] })
    if (n.href) void navigateHref(n.href)
  }
</script>

<Popover align="right" offset={8} class="w-80">
  {#snippet trigger(open)}
    <button
      type="button"
      aria-expanded={open}
      aria-label="Notifications"
      class="relative flex h-7 w-7 items-center justify-center rounded-md text-muted transition-colors duration-[120ms] dither-fill"
    >
      <Bell size={15} class="shrink-0" />
      {#if unread === null || unread > 0}
        <span
          title={unreadTitle}
          class="absolute right-0 top-0 font-mono text-[10px] font-medium leading-3 tracking-[0.05em] {unread === null ? 'text-[color:var(--theme-danger)]' : 'text-accent'}"
        >
          {unreadLabel}
        </span>
      {/if}
    </button>
  {/snippet}
  {#snippet content(close)}
    <div class="flex items-baseline justify-between px-3 pb-1.5 pt-2.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Notifications</span>
      {#if unread}
        <span class="font-mono text-[10px] text-accent">{unread} unread</span>
      {/if}
    </div>

    {#if rows.length === 0}
      <div class="px-3 py-6 text-center text-sm text-muted">All caught up.</div>
    {:else}
      <!-- max-h + overflow: the dropdown lists the last 50; a list that grew
           the page (or the strip) instead of scrolling would be a popover
           engine bug wearing a notifications badge. -->
      <div class="max-h-80 overflow-y-auto">
        {#each rows as n (n.id)}
          <button
            type="button"
            onclick={() => open(n, close)}
            class="flex w-full items-start gap-2.5 border-b border-line px-3 py-2 text-left transition-colors duration-[120ms] dither-fill last:border-b-0"
          >
            <!-- The dot is the read state: accent while the row waits, absent
                 once it has been seen. Read rows dim to muted so the eye
                 finds what is NEW, not what is merely recent. -->
            {#if !n.readAt}
              <StatusDot status="accent" class="mt-[7px]" />
            {:else}
              <span class="mt-[7px] h-1.5 w-1.5 shrink-0"></span>
            {/if}
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm {n.readAt ? 'text-muted' : 'font-medium text-fg'}">{n.title}</span>
              {#if n.body}
                <span class="mt-0.5 block truncate text-xs {n.readAt ? 'text-ink-dim' : 'text-muted'}">{n.body}</span>
              {/if}
              <span class="mt-0.5 block font-mono text-[10px] text-ink-dim">{relativeTime(n.createdAt)}</span>
            </span>
          </button>
        {/each}
      </div>
    {/if}

    {#if unread}
      <div class="border-t border-line px-2 py-1.5">
        <button
          type="button"
          onclick={() => {
            close()
            void markRead()
          }}
          class="flex w-full items-center rounded-md px-1.5 py-1 text-left text-sm text-muted transition-colors duration-[120ms] dither-fill"
        >
          Mark all read
        </button>
      </div>
    {/if}
  {/snippet}
</Popover>
