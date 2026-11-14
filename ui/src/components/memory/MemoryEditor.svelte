<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronLeft, History, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EditorTools from '@/components/editor/EditorTools.svelte'
  import InternalEditor, { DOC_MUSE_TITLE } from '@/components/editor/InternalEditor.svelte'
  import type { InternalEditorHandle } from '@/components/editor/internal-editor'
  import MemoryEntries from '@/components/memory/MemoryEntries.svelte'
  import { appendMemoryNote, memoryKey, saveMemory, useMemory } from '@/lib/memory'
  import { cn } from '@/lib/cn'
  import { useUiVoice } from '@/lib/ui-voice'

  // MEMORY, IN THE SHAPE IT IS KEPT IN — and the file it is kept in.
  //
  // Two views, and the ORDER is the point. Hermes stores a memory as a
  // §-delimited list of entries, so ENTRIES is the default: search it, fix one
  // entry, drop one entry (`MemoryEntries`, which owns the Memory/Profile
  // switch because the entry model is the same for both files). WHOLE FILE is
  // the escape hatch for everything a list cannot say — the workbench with its
  // version rail and its Muse, a "remember" line that appends a dated fact,
  // and one Save.
  //
  // The whole-file view deliberately stays MEMORY.md only. It is the hatch,
  // not a second editor to keep in sync: a person who needs to rewrite the
  // profile wholesale is a person who should see the list they are replacing.
  //
  // An earlier pane split this file into entries for a picker and got it
  // backwards: that was a READ-SIDE parse (`memory-doc.ts`, now gone) over
  // markdown headings, which could misread a row without protecting anything,
  // because every write still replaced the whole file. The split here is the
  // format the agent actually writes, and the writer round-trips it
  // (`memory-entries.ts`) — that is what makes editing a piece of it safe.
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
    flush = false,
    class: className,
  }: {
    id: string
    label: string
    canEdit?: boolean
    museContext: string
    note?: string
    offline?: { title: string; hint: string } | null
    /** No inset of its own: the pane sits on a page, not inside a panel. */
    flush?: boolean
    class?: string
  } = $props()

  const sans = useUiVoice() === 'sans'
  const inset = $derived(flush ? '' : 'px-4')

  const qc = useQueryClient()
  const query = useMemory(() => id)

  type View = 'entries' | 'file'
  const VIEWS: SegmentedOption<View>[] = [
    { id: 'entries', label: 'Entries', title: 'The list the agent keeps, one card per entry' },
    { id: 'file', label: 'Whole file', title: 'MEMORY.md as one document, with its version history' },
  ]
  let view = $state<View>('entries')

  let handle = $state<InternalEditorHandle | null>(null)
  let dirty = $state(false)
  let busy = $state(false)
  let fact = $state('')
  let remembering = $state(false)
  let failure = $state<unknown>(null)
  // History is one icon on the title line, right-aligned — same decision as
  // the record surface.
  let historyOpen = $state(false)
  let museOpen = $state(false)

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

<!-- Sans: the whole file is the way out of the list, so it is a quiet button
     at the end of the list's toolbar and not a second switch beside the file
     switch. -->
{#snippet openFile()}
  <Button variant="ghost" size="sm" class="-mr-3 shrink-0" onclick={() => (view = 'file')}>Open as one file</Button>
{/snippet}

{#if query.isError && offline}
  <EmptyState icon="◌" title={offline.title} hint={offline.hint} />
{:else if query.isError}
  <QueryError error={query.error} title="Can't reach the agent" onRetry={() => void query.refetch()} />
{:else}
  <!-- h-full, NO FLOOR: a floor taller than the frame overflows the tab's
       scroll region and unpins the footer (see RecordEditor). Short frames
       shrink the editor, which scrolls internally. -->
  <div class={cn('flex h-full flex-col', className)}>
    {#if sans}
      <!-- The entries view has its own toolbar, so this row is the whole-file
           view's alone. -->
      {#if view === 'file'}
        <div class={cn('mb-3 flex items-center gap-2', inset, !flush && 'pt-4')}>
          <Button variant="ghost" size="sm" class="-ml-2 gap-1 px-2" onclick={() => (view = 'entries')}>
            <ChevronLeft size={16} aria-hidden="true" />Entries
          </Button>
          <InfoTip text={tip} />
          <span class="ml-auto shrink-0 font-mono text-xs text-muted">MEMORY.md</span>
          <EditorTools
            class="ml-0"
            muse={canEdit ? { open: museOpen, onToggle: () => (museOpen = !museOpen), title: DOC_MUSE_TITLE } : undefined}
            history={{ open: historyOpen, onToggle: () => (historyOpen = !historyOpen) }}
          />
        </div>
      {/if}
    {:else}
      <div class="mb-3 flex items-center gap-1.5 px-4 pt-4">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span> <!-- voice-ok: mono branch, the agent manage dialog opened from Comms -->
        <InfoTip text={tip} />
        <!-- A mode switch, not a tab: the same document, read two ways. -->
        <Segmented options={VIEWS} value={view} onChange={(v) => (view = v)} class="ml-1" />
        <!-- The entries view names its own file (it switches between two), so the
             filename belongs to the whole-file view only. -->
        <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{label}{view === 'file' ? ' · MEMORY.md' : ''}</span> <!-- voice-ok: mono branch -->
        {#if view === 'file'}
          <!-- History, right-aligned on the title line — same place every record
               surface keeps it. No delete: there is no record without this file.
               It belongs to the whole-file view: the rail reverts the DOCUMENT,
               which is not an action an entry card can offer honestly. -->
          <IconButton title="Version history" active={historyOpen} onclick={() => (historyOpen = !historyOpen)}>
            <History size={14} />
          </IconButton>
        {/if}
      </div>
    {/if}

    {#if view === 'entries'}
      <!-- The list owns its scroll and its own read (one query key per file,
           so the Memory side is the very read this header is already waiting
           on — not a second request). -->
      <MemoryEntries {id} {canEdit} {flush} action={sans ? openFile : undefined} class={cn('min-h-0 flex-1', sans && !flush && 'pt-4')} />
      <!-- Never seed the editor from a read that has not landed: saving from an
           empty seed would replace the real memory with an empty file. The click
           is already answered — the header is here, the body is a skeleton. -->
    {:else if !query.data}
      <SkeletonRows rows={8} />
    {:else}
      {#if canEdit}
        <!-- Teach it one fact: appends a dated line to the document below. -->
        <div class={cn('mb-3 flex items-center gap-2', inset)}>
          <Input
            size="sm"
            bind:value={fact}
            onkeydown={(e) => e.key === 'Enter' && void remember()}
            placeholder={sans ? 'One fact the agent should keep' : 'Remember something: one fact the agent should keep'}
            disabled={remembering}
            class="flex-1"
          />
          {#if sans}
            <Button size="md" variant="outline" class="shrink-0" onclick={() => void remember()} disabled={remembering || !fact.trim()}>
              Remember
            </Button>
          {:else}
            <Button size="sm" class="shrink-0" onclick={() => void remember()} disabled={remembering || !fact.trim()}>
              <Plus size={13} /> Remember
            </Button>
          {/if}
        </div>
      {/if}

      <!-- min-h-0 so the editor takes what the chrome leaves; the floor lives
           on the root, where short windows turn it into a scrolling surface
           instead of a clipped one. -->
      <div class={cn('min-h-0 flex-1', inset)}>
        <InternalEditor
          bind:this={handle}
          bind:dirty
          bind:showHistory={historyOpen}
          bind:museOpen
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
        <QueryError variant="inline" class={cn('mt-3', inset)} error={failure} title="Could not save this memory" />
      {/if}

      {#if canEdit}
        <!-- Same fixed-height pinned menu as every record surface (see
             RecordEditor's footer for the 53px arithmetic). -->
        <div class={cn('flex h-[53px] shrink-0 items-center gap-2 border-t border-line', inset)}>
          {#if sans}
            <span class="ml-auto font-sans text-[13px] leading-5 text-muted">{dirty ? 'Unsaved changes' : ''}</span>
          {:else}
            <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{dirty ? 'Unsaved changes · ⌘S to save' : ''}</span> <!-- voice-ok: mono branch -->
          {/if}
          {#if dirty}
            <Button variant="ghost" size="sm" onclick={cancel}>
              Cancel
            </Button>
          {/if}
          <!-- In the settings voice Save is there only while there is something to save. -->
          {#if !sans || dirty || busy}
            <Button size="sm" onclick={() => void saveAll()} disabled={busy || !dirty}>
              {busy ? 'Saving' : 'Save'}
            </Button>
          {/if}
        </div>
      {/if}
    {/if}
  </div>
{/if}
