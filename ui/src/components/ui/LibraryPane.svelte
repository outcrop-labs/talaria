<script lang="ts" module>
  import type { Snippet } from 'svelte'
  import type { QueryErrorProps } from '@/components/ui/query-state'

  /** One labelled run of items in the picker. A single group with no `label`
   *  is a plain flat list — that is the common case, and it must not cost the
   *  caller a header it did not ask for. */
  export interface LibraryGroup<T> {
    label?: string
    items: T[]
    /** Shown in place of the rows when this group is empty. Omit to render
     *  nothing, which is right for a group that is merely one of several. */
    empty?: string
  }

  export interface LibraryPaneProps<T> {
    groups: LibraryGroup<T>[]
    /** Stable identity for a row — selection, keying and menus all use it. */
    idOf: (item: T) => string
    labelOf: (item: T) => string
    /** CONTROLLED. The caller owns the selection, which is what lets a
     *  URL-driven view (`?t=`, `/studio?a=`) and a local-state one share this
     *  component without either bending to the other. */
    selectedId: string | null
    onSelect: (item: T) => void
    /** Title over the picker. Omit for a bare list. */
    title?: string
    /** First load, from `listQuery(...).pending`. Draws row-shaped skeletons. */
    pending?: boolean
    /** From `listQuery(...).notice` — the failure, already shaped and wired to
     *  Retry. Rendered INSTEAD of the rows, because an empty list beside an
     *  error reads as "and also there are none". */
    notice?: QueryErrorProps | null
    /** Right-click on a row. The caller owns its own `useContextMenu()` and
     *  renders `<ContextMenu>` itself — this only forwards the event, so the
     *  menu implementation stays out of the component. */
    onRowMenu?: (e: MouseEvent, item: T) => void
    /** Width of the picker column. */
    listWidth?: string
    /** WHICH SURFACE THIS SITS ON, which is not decoration — Mercury's layers
     *  are what tell a reader one region is inside another.
     *
     *  `panel` (default) is a panel on the app ground: correct for a page.
     *  `well` is an INSET WELL on a panel — the same `bg-surface` that
     *  InternalEditorModal and the chat composer use — and it is what a pane
     *  inside a modal or a section needs. Leaving such a pane on `panel` paints
     *  panel on panel: the fill matches its container exactly, so the whole
     *  surface reads as a bare outline with nothing behind it.
     *  `bare` has no chrome at all, for a caller supplying its own frame. */
    surface?: 'panel' | 'well' | 'bare'
    class?: string
    /** ADDING A NEW ONE, which is the pane's job rather than each caller's.
     *
     *  There were three affordances for this across three views: a raw
     *  Input+Button pinned under the list, an `InlineCreate`, and a bare `+`
     *  in the header that opened an empty editor. Same act, three controls,
     *  three positions, and only one of them told you what it would create.
     *
     *  Given `onCreate`, the pane renders the one shared control in the picker
     *  footer: a "+ <label>" button that expands into a named input, Enter or
     *  blur to submit, Escape to cancel. The caller gets the trimmed, non-empty
     *  name and decides what a new record made of it looks like — which may be
     *  a saved row (Templates, Teams) or an unsaved draft in the editor (the
     *  role library). That difference belongs to the caller; the control does
     *  not change shape for it. */
    onCreate?: (name: string) => void | Promise<void>
    /** The create control's TOOLTIP and ACCESSIBLE NAME — not visible text.
     *
     *  The button itself is a bare `+`, because the footer of a library
     *  already says what a new one would be and spelling it out tells the
     *  reader what they can see. The name is a different audience: a screen
     *  reader gets no help from placement, so this one SHOULD be specific.
     *  Terse on screen, precise in the accessibility tree. */
    createLabel?: string
    createPlaceholder?: string
    /** Row body override, for rows that need more than a name. */
    row?: Snippet<[T, boolean]>
    /** Trailing per-row control (a delete affordance). Reveals on row hover. */
    rowAction?: Snippet<[T]>
    /** The right pane, when something is selected. */
    detail?: Snippet
    /** The right pane, when nothing is. */
    empty?: Snippet
  }
</script>

<script lang="ts" generics="T">
  import InlineCreate from '@/components/ui/InlineCreate.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { listStagger } from '@/lib/motion'

  // A LIBRARY ON THE LEFT, THE THING YOU PICKED ON THE RIGHT.
  //
  // This shape had five independent implementations — Templates, agent Role
  // templates, Studio, the Teams dialog, and the Rail views — which is five
  // answers to the same questions: does the list scroll with the page or
  // inside itself, is a failed read an empty list, does the picker sit in a
  // Panel or a bare grid, what does "nothing selected" look like. They had
  // drifted into genuinely different behaviour rather than merely different
  // markup, and the divergence was invisible until you used two of them in a
  // row.
  //
  // THE PANE OWNS ITS OWN SCROLL, both sides, independently. That is the house
  // rule for a view whose content outgrows the viewport — the picker must not
  // push the editor down the page, and neither should grow a second scrollbar
  // under the shell. Callers give it a height by placing it in a sized
  // container; it fills what it is given.
  //
  // SELECTION IS THE CALLER'S. Templates keeps it in the URL, the Teams dialog
  // in a local `$state`; a component that insisted on one would have kept the
  // other on its own copy of this.
  //
  // It takes `pending` and `notice` from `listQuery` rather than a raw query,
  // so "the read failed" arrives already shaped and cannot be quietly dropped
  // on the way in.
  let {
    groups,
    idOf,
    labelOf,
    selectedId,
    onSelect,
    title,
    pending = false,
    notice = null,
    onRowMenu,
    listWidth = 'w-64',
    surface = 'panel',
    class: className,
    onCreate,
    createLabel = 'New',
    createPlaceholder = 'Name',
    row,
    rowAction,
    detail,
    empty,
  }: LibraryPaneProps<T> = $props()

  const total = $derived(groups.reduce((n, g) => n + g.items.length, 0))
  // A group with no label and no rows contributes nothing; one WITH a label
  // still announces itself, because "Your organization (none yet)" is
  // information and a silently missing section is not.
  const visible = $derived(groups.filter((g) => g.label || g.items.length || g.empty))
</script>

<!-- The frame is a snippet so `surface` picks the chrome without duplicating
     the two panes. -->
{#snippet panes()}
  <!-- ── The picker ──────────────────────────────────────────────────────── -->
  <div class={cn('flex shrink-0 flex-col border-r border-line', listWidth)}>
    {#if title}
      <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{title}</span>
      </div>
    {/if}

    <div class="min-h-0 flex-1 overflow-y-auto p-2">
      {#if pending}
        <!-- Row-shaped, at the row's own dimensions — a generic bar here would
             make the list jump when the real rows land. -->
        <div aria-hidden="true">
          {#each [0, 1, 2, 3, 4] as i (i)}
            <div class="px-2 py-1.5">
              <Skeleton
                class={`h-3 rounded-full ${['w-32', 'w-24', 'w-40', 'w-28', 'w-36'][i % 5]}`}
                delay={i * 0.12}
              />
            </div>
          {/each}
        </div>
      {:else if notice}
        <QueryError {...notice} />
      {:else if total === 0 && visible.length === 0}
        <p class="px-2 py-3 font-sans text-xs text-muted">None yet.</p>
      {:else}
        {#each visible as group, gi (group.label ?? gi)}
          {#if group.label}
            <div class="px-2 pb-1 pt-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {group.label}
            </div>
          {/if}
          {#if group.items.length === 0}
            {#if group.empty}<p class="px-2 pb-1 font-sans text-xs text-muted">{group.empty}</p>{/if}
          {:else}
            <div use:listStagger>
              {#each group.items as item (idOf(item))}
                {@const id = idOf(item)}
                {@const active = selectedId === id}
                <div class="group/row flex items-center gap-1">
                  <button
                    type="button"
                    onclick={() => onSelect(item)}
                    oncontextmenu={onRowMenu ? (e) => onRowMenu(e, item) : undefined}
                    aria-current={active ? 'true' : undefined}
                    class={cn(
                      'min-w-0 flex-1 truncate rounded-md px-2 py-1.5 text-left font-sans text-[13px] transition-colors',
                      active ? 'bg-raised text-fg' : 'text-muted hover:bg-hover hover:text-fg',
                    )}
                  >
                    {#if row}{@render row(item, active)}{:else}{labelOf(item)}{/if}
                  </button>
                  {#if rowAction}
                    <div class="shrink-0 opacity-0 transition-opacity group-hover/row:opacity-100">
                      {@render rowAction(item)}
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        {/each}
      {/if}
    </div>

    {#if onCreate}
      <div class="shrink-0 border-t border-line px-3 py-3">
        <!-- A bare `+`, left-aligned. The footer of a library already says
             what a new one would be; a button that spells it out is telling
             the reader something they can see. The label survives as the
             tooltip and the accessible name. -->
        <InlineCreate
          icon
          label={createLabel}
          placeholder={createPlaceholder}
          onSubmit={(name) => void onCreate(name)}
        />
      </div>
    {/if}
  </div>

  <!-- ── The detail ──────────────────────────────────────────────────────── -->
  <div class="flex min-h-0 min-w-0 flex-1 flex-col">
    {#if selectedId && detail}
      {@render detail()}
    {:else}
      <div class="grid min-h-0 flex-1 place-items-center overflow-y-auto p-8">
        {@render empty?.()}
      </div>
    {/if}
  </div>
{/snippet}

{#if surface === 'bare'}
  <div class={cn('flex min-h-0 overflow-hidden', className)}>{@render panes()}</div>
{:else if surface === 'well'}
  <div class={cn('flex min-h-0 overflow-hidden rounded-lg border border-line bg-surface', className)}>
    {@render panes()}
  </div>
{:else}
  <Panel class={cn('flex min-h-0 overflow-hidden p-0', className)}>{@render panes()}</Panel>
{/if}
