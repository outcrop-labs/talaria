<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { History, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import InternalEditor from '@/components/editor/InternalEditor.svelte'
  import type { InternalEditorHandle } from '@/components/editor/internal-editor'
  import { appendMemoryNote, memoryKey, saveMemory, useMemory } from '@/lib/memory'
  import { cn } from '@/lib/cn'

  // MEMORY, EDITED AS WHAT IT IS: one file.
  //
  // The pane that used to live here split MEMORY.md into entries for a
  // scanable sidebar — a READ-SIDE parse (`memory-doc.ts`, now gone) feeding a
  // picker, with "Edit all" swapping the whole pane for a workbench. The split
  // cost more than it gave: the file is the agent's own curated record, a
  // person's edits are whole-file edits, and a parse that misread a row cost a
  // confused picker without ever protecting the file. The editor IS the memory
  // now — the workbench with its version rail and its Muse, a "remember" line
  // that appends a dated fact, and one Save.
  //
  // The remember line appends to WHAT YOU SEE — the editor's live text — and
  // stages the result straight back, so a fact just appended can never be
  // clobbered by a Save from an editor still showing the pre-append file.
  // That was the old pane's hazard, in both directions.
  let {
    id,
    label,
    canEdit = true,
    museContext,
    /** Extra sentence for the tooltip — the fleet names the container. */
    note,
    /** Shown INSTEAD of an error when the owner is simply not running, which is
     *  an explainable state rather than a fault. */
    offline,
    class: className,
  }: {
    id: string
    label: string
    canEdit?: boolean
    museContext: string
    note?: string
    offline?: { title: string; hint: string } | null
    class?: string
  } = $props()

  const qc = useQueryClient()
  const query = useMemory(() => id)

  let handle = $state<InternalEditorHandle | null>(null)
  let dirty = $state(false)
  let busy = $state(false)
  let fact = $state('')
  let remembering = $state(false)
  let failure = $state<unknown>(null)
  // History is one icon on the title line, right-aligned — same decision as
  // the record surface.
  let historyOpen = $state(false)

  const refresh = () => qc.invalidateQueries({ queryKey: memoryKey(id) })

  const save = async (content: string) => {
    await saveMemory(id, content)
    await refresh()
  }

  // The editor's own Save; `busy` is the footer button's state. The workbench's
  // internal ⌘S takes the same path through its own handler.
  const saveAll = async () => {
    busy = true
    failure = null
    try {
      await save(handle?.getMarkdown() ?? query.data?.content ?? '')
      dirty = false
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }

  const cancel = () => {
    handle?.restoreDoc(query.data?.content ?? '')
    dirty = false
  }

  const remember = async () => {
    const text = fact.trim()
    if (!text || !query.data || remembering) return
    remembering = true
    failure = null
    try {
      const content = await appendMemoryNote(id, handle?.getMarkdown() ?? query.data.content, text, new Date().toISOString().slice(0, 10))
      // The append already persisted this exact text — stage it as the saved
      // state, not as unsaved work.
      handle?.restoreDoc(content)
      await refresh()
      fact = ''
    } catch (e) {
      failure = e
    } finally {
      remembering = false
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
  <!-- h-full, NO FLOOR: a floor taller than the frame overflows the tab's
       scroll region and unpins the footer (see RecordEditor). Short frames
       shrink the editor, which scrolls internally. -->
  <div class={cn('flex h-full flex-col', className)}>
    <div class="mb-3 flex items-center gap-1.5 px-4 pt-4">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span>
      <InfoTip text={tip} />
      <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{label} · MEMORY.md</span>
      <!-- History, right-aligned on the title line — same place every record
           surface keeps it. No delete: there is no record without this file. -->
      <IconButton title="Version history" active={historyOpen} onclick={() => (historyOpen = !historyOpen)}>
        <History size={14} />
      </IconButton>
    </div>

    <!-- Never seed the editor from a read that has not landed: saving from an
         empty seed would replace the real memory with an empty file. The click
         is already answered — the header is here, the body is a skeleton. -->
    {#if !query.data}
      <SkeletonRows rows={8} />
    {:else}
      {#if canEdit}
        <!-- Teach it one fact: appends a dated line to the document below. -->
        <div class="mb-3 flex items-center gap-2 px-4">
          <Input
            size="sm"
            bind:value={fact}
            onkeydown={(e) => e.key === 'Enter' && void remember()}
            placeholder="Remember something: one fact the agent should keep"
            disabled={remembering}
            class="flex-1"
          />
          <Button size="sm" class="shrink-0" onclick={() => void remember()} disabled={remembering || !fact.trim()}>
            <Plus size={13} /> Remember
          </Button>
        </div>
      {/if}

      <!-- min-h-0 so the editor takes what the chrome leaves; the floor lives
           on the root, where short windows turn it into a scrolling surface
           instead of a clipped one. -->
      <div class="min-h-0 flex-1 px-4">
        <InternalEditor
          bind:this={handle}
          bind:dirty
          bind:showHistory={historyOpen}
          value={query.data.content}
          editable={canEdit}
          onSave={save}
          history={{ kind: 'memory', id }}
          muse={{ kind: 'memory', context: museContext }}
          fill
          withActions={false}
        />
      </div>

      {#if failure}
        <QueryError variant="inline" class="mt-3 px-4" error={failure} title="Could not save this memory" />
      {/if}

      {#if canEdit}
        <!-- Same fixed-height pinned menu as every record surface (see
             RecordEditor's footer for the 53px arithmetic). -->
        <div class="flex h-[53px] shrink-0 items-center gap-2 border-t border-line px-4">
          <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{dirty ? 'Unsaved changes · ⌘S to save' : ''}</span>
          {#if dirty}
            <Button variant="ghost" size="sm" onclick={cancel}>
              Cancel
            </Button>
          {/if}
          <Button size="sm" onclick={() => void saveAll()} disabled={busy || !dirty}>
            {busy ? 'Saving' : 'Save'}
          </Button>
        </div>
      {/if}
    {/if}
  </div>
{/if}
