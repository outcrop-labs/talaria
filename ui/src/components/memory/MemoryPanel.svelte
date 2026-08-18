<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { appendMemoryNote, memoryKey, saveMemory, useMemory } from '@/lib/memory'
  import { parseMemory, type MemoryEntry } from '@/lib/memory-doc'

  // WHAT AN AGENT REMEMBERS — one MEMORY.md, read live from its container,
  // shown as the library of facts it actually is.
  //
  // The document is one file and the panel used to render it as one scroll,
  // which is fine at ten lines and unusable at the two hundred an agent
  // accumulates in a month. The picker splits it into entries so it can be
  // scanned; see `memory-doc.ts` for why that split is READ-SIDE ONLY —
  // appending appends and editing replaces the whole file, so a parse that
  // misreads something costs an odd picker row, never a corrupted memory.
  //
  // It also existed twice (personal assistant, fleet agent) with divergent
  // query keys and saves that ignored their own response; one client and one
  // panel now, with refused writes surfaced.
  let {
    id,
    label,
    canEdit = true,
    museContext,
    /** Extra sentence for the tooltip — the fleet names the container. */
    note,
    /** The editor opens from inside another modal. */
    nested = false,
    /** Shown INSTEAD of an error when the owner is simply not running, which is
     *  an explainable state rather than a fault. */
    offline,
    surface = 'panel',
    class: className,
  }: {
    id: string
    label: string
    canEdit?: boolean
    museContext: string
    note?: string
    nested?: boolean
    offline?: { title: string; hint: string } | null
    /** `well` when this sits inside a modal or a section — see LibraryPane. */
    surface?: 'panel' | 'well' | 'bare'
    class?: string
  } = $props()

  const qc = useQueryClient()
  const query = useMemory(() => id)
  let editing = $state(false)
  let busy = $state(false)
  let selected = $state<string | null>(null)
  let failure = $state<unknown>(null)

  const entries = $derived(parseMemory(query.data?.content ?? ''))
  const current = $derived(entries.find((e) => e.id === selected) ?? null)

  const refresh = () => qc.invalidateQueries({ queryKey: memoryKey(id) })

  const save = async (content: string) => {
    busy = true
    failure = null
    try {
      await saveMemory(id, content)
      await refresh()
    } catch (e) {
      // Previously invisible: both copies discarded the response, so a refused
      // write closed the editor as though it had landed.
      failure = e
    } finally {
      busy = false
    }
  }

  // The pane's `+` teaches it one fact. Appending before the read lands would
  // write over the whole document with a single line.
  const remember = async (fact: string) => {
    if (!query.data) return
    busy = true
    failure = null
    try {
      await appendMemoryNote(id, query.data.content, fact, new Date().toISOString().slice(0, 10))
      await refresh()
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }

  const tip = $derived(
    `${note ? `${note} ` : ''}The agent curates this itself, so a concurrent agent write can win over a manual edit. Every save is snapshotted and revertible.`,
  )
</script>

{#if query.isError && offline}
  <EmptyState icon="◌" title={offline.title} hint={offline.hint} />
{:else if query.isError}
  <QueryError error={query.error} title="Can't reach the agent" onRetry={() => void query.refetch()} />
{:else}
  <div class={className}>
    <div class="mb-3 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span>
      <InfoTip text={tip} />
      {#if canEdit}
        <!-- Whole-document editing stays a separate act: the picker reads the
             file, the editor owns writing it. -->
        <Button size="sm" variant="outline" class="ml-auto shrink-0" disabled={!query.data} onclick={() => (editing = true)}>
          Edit all
        </Button>
      {/if}
    </div>

    <LibraryPane
      groups={[{ items: entries }]}
      idOf={(e: MemoryEntry) => e.id}
      labelOf={(e: MemoryEntry) => e.title}
      selectedId={selected}
      onSelect={(e: MemoryEntry) => (selected = e.id)}
      pending={!query.data}
      onCreate={canEdit ? remember : undefined}
      createLabel="Remember something"
      createPlaceholder="One fact the agent should keep"
      {surface}
      class="h-[26rem]"
    >
      {#snippet empty()}
        <EmptyState
          icon="◌"
          title={entries.length ? 'Nothing selected' : 'Nothing remembered yet'}
          hint={entries.length
            ? 'Pick one on the left.'
            : canEdit
              ? 'It writes things down as you work together — or teach it one now.'
              : 'It writes things down as it works.'}
        />
      {/snippet}

      {#snippet detail()}
        {#if current}
          <div class="min-h-0 flex-1 overflow-y-auto p-6 text-sm">
            <Markdown children={current.body} />
          </div>
        {/if}
      {/snippet}
    </LibraryPane>

    {#if failure}
      <QueryError variant="inline" class="mt-3" error={failure} title="Could not save this memory" />
    {/if}

    {#if editing}
      <InternalEditorModal
        open
        {nested}
        onClose={() => (editing = false)}
        title={`${label} · MEMORY.md`}
        subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
        value={query.data?.content ?? ''}
        editable={canEdit}
        saving={busy}
        onSave={save}
        history={{ kind: 'memory', id }}
        muse={{ kind: 'memory', context: museContext }}
      />
    {/if}
  </div>
{/if}
