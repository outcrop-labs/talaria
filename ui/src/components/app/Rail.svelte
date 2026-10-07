<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { PanelLeftClose, PanelLeftOpen } from '@lucide/svelte'
  import { readText, writeText } from '@/lib/persist'

  /** The list rail — ALWAYS on the left, w-72 open. (See RailSurface.svelte
   *  for the page-archetype grammar this belongs to.) No title: naming the
   *  view is the top strip's job now. The header bar exists only to carry
   *  actions, and a rail with none of those starts straight at its content.
   *
   *  COLLAPSING IS OPT-IN (`collapseKey`), not automatic. Every rail surface
   *  shares this component, so a collapse everybody got would change Comms,
   *  Plan, Research and Boards in one commit without anyone asking. A surface
   *  that wants it names its own storage key and gets it; the rest render
   *  exactly as before, down to the absent button.
   *
   *  The key is per SURFACE rather than per user-and-surface because the
   *  preference is about this browser's window, the same reasoning
   *  `useStickyAgent` uses for its own key. */
  let {
    actions,
    children,
    footer,
    collapseKey,
    collapsedLabel = 'list',
    class: className,
  }: {
    /** Compact IconButtons; never a giant labeled button. Right-aligned —
     *  they sat right when a title still filled the left. */
    actions?: Snippet
    children: Snippet
    /** Pinned under the scrolling list, always visible (Comms puts your
     *  status there). Absent: the list runs to the bottom as before. */
    footer?: Snippet
    /** Opt in to collapsing, and name where the preference is remembered
     *  (`talaria.rail.<key>`). Omit it and the rail cannot collapse at all. */
    collapseKey?: string
    /** What the collapsed strip says, rotated. Keep it one short word — it is
     *  read sideways. */
    collapsedLabel?: string
    class?: string
  } = $props()

  const storageKey = $derived(collapseKey ? `talaria.rail.${collapseKey}` : null)

  // Read ONCE at init from the key this rail was given. Not a $derived over
  // storage: the toggle below is the only thing that changes it afterwards,
  // and re-reading would fight the click.
  let collapsed = $state(false)
  $effect(() => {
    // Runs on mount (and if a surface ever swapped its key), never on toggle —
    // `collapsed` is deliberately not read here.
    const key = storageKey
    if (!key) return
    collapsed = readText(key) === '1'
  })

  const toggle = () => {
    collapsed = !collapsed
    if (storageKey) writeText(storageKey, collapsed ? '1' : null)
  }
</script>

{#if collapseKey && collapsed}
  <!-- The collapsed strip is a BUTTON, all of it: a 12px-wide rail with a
       separate small target in it would be a worse click than the whole
       column, and there is nothing else in here to hit. -->
  <aside class={cn('flex h-full w-10 shrink-0 flex-col border-r border-line bg-sidebar', className)}>
    <button
      type="button"
      onclick={toggle}
      title={`Show the ${collapsedLabel}`}
      aria-label={`Show the ${collapsedLabel}`}
      aria-expanded="false"
      class="group flex h-full w-full flex-col items-center gap-3 py-3 text-muted transition-colors hover:bg-raised hover:text-fg"
    >
      <PanelLeftOpen size={15} class="shrink-0" />
      <!-- Sideways, so a collapsed rail still says what it is holding rather
           than reading as decorative chrome. -->
      <span
        class="font-mono text-[10px] uppercase tracking-[0.12em]"
        style="writing-mode: vertical-rl; text-orientation: mixed;"
      >
        {collapsedLabel}
      </span>
    </button>
  </aside>
{:else}
  <aside class={cn('flex h-full w-72 shrink-0 flex-col border-r border-line bg-sidebar', className)}>
    {#if actions || collapseKey}
      <div class="flex h-12 shrink-0 items-center gap-1.5 border-b border-line px-4">
        {#if collapseKey}
          <!-- LEFT of the actions, and the only thing on that side: it is
               about the rail itself, not about what the rail lists. -->
          <button
            type="button"
            onclick={toggle}
            title={`Hide the ${collapsedLabel}`}
            aria-label={`Hide the ${collapsedLabel}`}
            aria-expanded="true"
            class="-ml-1 grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-raised hover:text-fg"
          >
            <PanelLeftClose size={15} />
          </button>
        {/if}
        {#if actions}
          <div class="ml-auto flex items-center gap-1.5">{@render actions()}</div>
        {/if}
      </div>
    {/if}
    <div class="min-h-0 flex-1 overflow-y-auto p-3">{@render children()}</div>
    {#if footer}
      <div class="shrink-0 border-t border-line p-2">{@render footer()}</div>
    {/if}
  </aside>
{/if}
