<script lang="ts">
  // ⌘K — JUMP TO A SESSION. One line to mount, because the shortcut is the
  // feature: a host that had to own the key handler, the open state and the
  // modal would be three places the palette could be half-wired, and the rail
  // it sits beside is already the long way round.
  //
  // So this component owns all three. The host hands it the sessions it can
  // see and what to do with a pick, and nothing else — which also means a
  // second surface can mount it without copying any wiring.
  import Modal from '@/components/ui/Modal.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import { cn } from '@/lib/cn'
  import type { Conversation } from '@/lib/conversations.svelte'
  import { filterSessions, nextIndex } from './session-palette'

  let {
    conversations,
    onSelect,
  }: {
    conversations: Conversation[]
    /** The pick, by id. Navigating there is the host's — the palette does not
     *  know which surface a session opens on. */
    onSelect: (id: string) => void
  } = $props()

  let open = $state(false)
  let query = $state('')
  let index = $state(0)
  let rowEls = $state<Array<HTMLButtonElement | null>>([])

  const results = $derived(filterSessions(conversations, query))

  // A stale index outlives its row: type one more letter and the list can be
  // shorter than where the cursor was standing.
  const cursor = $derived(results.length === 0 ? 0 : Math.min(index, results.length - 1))

  /** Focus sitting in a field somebody is TYPING IN. ⌘K belongs to that field
   *  there (the composer's own, a search box's), and stealing it mid-sentence
   *  is the one way a global shortcut becomes a bug. The palette's own field
   *  is not this case — while it is open the key is already ours, and
   *  PopSearch puts the caret back in the field. */
  const inTextField = (target: EventTarget | null): boolean =>
    target instanceof Element && target.closest('input,textarea,[contenteditable]') !== null

  const show = () => {
    query = ''
    index = 0
    open = true
  }

  const pick = (id: string) => {
    open = false
    onSelect(id)
  }

  const onKeydown = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      // Open, or stay open: ⌘K twice is a person reaching for the palette they
      // are already in, never a toggle shut.
      if (open) {
        e.preventDefault()
        return
      }
      if (inTextField(e.target)) return
      e.preventDefault()
      show()
      return
    }
    if (!open) return
    // Esc is Modal's (one dialog, one dismissal path).
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      index = nextIndex(cursor, e.key === 'ArrowDown' ? 1 : -1, results.length)
    } else if (e.key === 'Enter') {
      const hit = results[cursor]
      if (!hit) return
      e.preventDefault()
      pick(hit.id)
    }
  }

  // Keep the highlighted row on screen when the arrows walk past the edge of
  // the scroll box. `nearest` so a mouse-driven list never jumps under the
  // pointer.
  $effect(() => {
    if (!open) return
    rowEls[cursor]?.scrollIntoView({ block: 'nearest' })
  })
</script>

<svelte:window onkeydown={onKeydown} />

<!-- No title: the field says what this is, and a header over a one-field
     dialog is a line of chrome between the caret and the list. -->
<Modal {open} onClose={() => (open = false)} width="max-w-lg" padded={false}>
  <div class="flex max-h-[60vh] min-h-0 flex-col p-3">
    <PopSearch
      value={query}
      onChange={(v) => {
        query = v
        index = 0
      }}
      placeholder="Jump to a session"
    />
    {#if results.length === 0}
      <EmptyState
        variant="compact"
        icon="◇"
        title={conversations.length === 0 ? 'No sessions yet' : 'Nothing matches'}
      />
    {:else}
      <div class="-mx-1 min-h-0 flex-1 overflow-y-auto px-1">
        {#each results as c, i (c.id)}
          <button
            bind:this={rowEls[i]}
            type="button"
            onclick={() => pick(c.id)}
            onmousemove={() => (index = i)}
            class={cn(
              'flex w-full min-w-0 items-center gap-1.5 rounded-md px-2 py-1.5 text-left font-sans text-sm transition-colors',
              i === cursor ? 'bg-raised text-fg' : 'text-muted',
            )}
          >
            <!-- The rail's row, unchanged: the same anatomy in the palette as
                 in the list it is standing in for. The group rides in the
                 meta line's chip slot — it is why a row matched a query that
                 is nowhere in its title. -->
            <SessionRowBody
              conv={c}
              active={i === cursor}
              pinned={c.pinned}
              chip={c.group ?? undefined}
            />
          </button>
        {/each}
      </div>
    {/if}
  </div>
</Modal>
