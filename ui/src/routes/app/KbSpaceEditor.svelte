<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { History, ListTree, Maximize2, Minimize2, Plus, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { copyAppLink, useContextMenu } from '@/components/ui/context-menu.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import PermissionsModal from '@/components/kb/PermissionsModal.svelte'
  import { updateSpace, useSpace } from '@/lib/kb'
  import { fade, fly, QUICK } from '@/lib/motion'
  import KbDocPageSkeleton from './KbDocPageSkeleton.svelte'
  import KbHistoryRail from './KbHistoryRail.svelte'
  import KbMuseBar from './KbMuseBar.svelte'
  import KbSettingsMenu from './KbSettingsMenu.svelte'
  import KbTocPanel from './KbTocPanel.svelte'
  import KbVisibilityIcon from './KbVisibilityIcon.svelte'
  import { docSearch, editorShell, parseHeadings, useIsOwner } from './knowledge.svelte'

  // ── Space overview (top-level folder = document) ──────────────────────────
  let { spaceId, onNewDoc, onDeleted }: { spaceId: string; onNewDoc: () => void; onDeleted: () => void } = $props()

  const qc = useQueryClient()
  const spaceQuery = useSpace(() => spaceId)
  const space = $derived(spaceQuery.data)
  let editorRef = $state<RichEditor | null>(null)
  let bodyRef = $state<HTMLDivElement | null>(null)
  let name = $state('')
  let emojiOpen = $state(false)
  let fullscreen = $state(false)
  let shareOpen = $state(false)
  let showToc = $state(false)
  let showHistory = $state(false)
  let seed = $state(0)
  let mode = $state<'read' | 'edit'>('read')
  let museSel = $state<{ text: string; source: 'read' | 'editor' } | null>(null)
  const spaceMenu = useContextMenu()
  const isOwner = useIsOwner(() => space)
  let initMode = false
  const headings = $derived(parseHeadings(space?.body ?? ''))
  const scrollToHeading = (index: number) => {
    bodyRef?.querySelectorAll('.tiptap h1, .tiptap h2, .tiptap h3')[index]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  $effect(() => {
    if (space) name = space.name
    if (space && !initMode) {
      initMode = true
      mode = space.body.trim() ? 'read' : 'edit'
    }
  })

  const save = async (patch: Parameters<typeof updateSpace>[1]) => {
    await updateSpace(spaceId, patch)
    await qc.invalidateQueries({ queryKey: ['kb-space', spaceId] })
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
  }
  const saveBody = () => save({ name: name.trim() || 'Untitled', body: editorRef?.getMarkdown() ?? space?.body ?? '' })
</script>

<svelte:window
  onkeydown={(e) => {
    if (fullscreen && e.key === 'Escape') fullscreen = false
  }}
/>

<!-- `if (!space)` used to cover all three answers at once, so a 500 and a
     deleted folder both shimmered nine placeholder bars for ever. `useSpace`
     rejects on failure and resolves to `null` for a real 404 — the two ARE
     distinguishable here, this branch simply never looked. -->
{#if spaceQuery.isError && space === undefined}
  <div in:fade={{ duration: 150 }}>
    <QueryError
      error={spaceQuery.error}
      title="Could not load this folder"
      onRetry={() => void spaceQuery.refetch()}
    />
  </div>
{:else if space === null}
  <div in:fade={{ duration: 150 }}>
    <EmptyState icon="⧉" title="Folder not found" hint="It may have been deleted, or you don’t have access." />
  </div>
{:else if !space}
  <KbDocPageSkeleton bars={9} />
{:else}
  <div class={editorShell(fullscreen)}>
    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
      <div class="relative shrink-0">
        <button type="button" onclick={() => (emojiOpen = !emojiOpen)} class="rounded-md px-1 text-xl leading-none transition-colors hover:bg-hover" title="Set icon">
          {space.icon ?? '📚'}
        </button>
        {#if emojiOpen}
          <EmojiPicker
            onPick={(e) => {
              void save({ icon: e })
              emojiOpen = false
            }}
            onClear={() => {
              void save({ icon: null })
              emojiOpen = false
            }}
            onClose={() => (emojiOpen = false)}
          />
        {/if}
      </div>
      {#if mode === 'edit'}
        <Input
          bind:value={name}
          onblur={() => name.trim() && name !== space.name && void save({ name: name.trim() })}
          onkeydown={inlineEditKeys(() => (name = space.name))}
          class="min-w-0 flex-1 border-0 bg-transparent text-xl font-semibold focus:border-0"
          placeholder="Space name"
        />
      {:else}
        <div class="min-w-0 flex-1">
          <h1 class="truncate font-sans text-lg font-semibold text-fg">{space.name}</h1>
          <div class="truncate font-mono text-[10px] uppercase tracking-[0.05em] text-muted">space overview</div>
        </div>
      {/if}
      <Chip class="font-normal">Folder</Chip>
      <Segmented
        size="xs"
        options={[{ id: 'read', label: 'Read' }, { id: 'edit', label: 'Edit' }] as const}
        value={mode}
        onChange={(m) => {
          if (m === 'read' && mode === 'edit') void saveBody()
          mode = m
        }}
      />
      <Button variant="ghost" size="sm" class="shrink-0" title="Share & permissions" onclick={() => (shareOpen = true)}>
        <KbVisibilityIcon v={space.visibility} /> <span class="ml-1.5 capitalize">{space.visibility}</span>
      </Button>
      <span class="mx-0.5 h-4 shrink-0 border-l border-line-subtle"></span>
      <Button variant={showToc ? 'outline' : 'ghost'} size="sm" class="shrink-0" title="Table of contents" onclick={() => (showToc = !showToc)}>
        <ListTree size={14} />
      </Button>
      <Button variant="ghost" size="sm" class="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onclick={() => (fullscreen = !fullscreen)}>
        {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </Button>
      <Button variant="outline" size="sm" class="shrink-0" onclick={onNewDoc}>
        <Plus size={13} class="mr-1" /> New
      </Button>
      <KbSettingsMenu
        items={[
          { label: showHistory ? 'Hide history' : 'Version history', icon: History, onClick: () => (showHistory = !showHistory) },
          {
            label: 'Delete folder',
            icon: Trash2,
            danger: true,
            onClick: async () => {
              if (await confirm({ title: 'Delete folder', message: `Delete "${space.name}" and all its docs?`, confirmLabel: 'Delete', danger: true })) onDeleted()
            },
          },
        ]}
      />
    </div>
    <div bind:this={bodyRef} class="flex min-h-0 flex-1">
      {#if mode === 'edit'}
        <div
          class="flex min-w-0 flex-1 flex-col"
          role="presentation"
          oncontextmenu={(e) => {
            const sel = editorRef?.getSelectionText() ?? ''
            const inTable = editorRef?.isInTable() ?? false
            spaceMenu.openMenu(e, [
              { label: 'Bold', disabled: !sel, onSelect: () => editorRef?.toggleMark('bold') },
              { label: 'Italic', disabled: !sel, onSelect: () => editorRef?.toggleMark('italic') },
              { label: 'Strikethrough', disabled: !sel, onSelect: () => editorRef?.toggleMark('strike') },
              { label: 'Inline code', disabled: !sel, onSelect: () => editorRef?.toggleMark('code') },
              'sep',
              ...(inTable
                ? ([
                    { label: 'Add row below', onSelect: () => editorRef?.tableCommand('addRowAfter') },
                    { label: 'Add row above', onSelect: () => editorRef?.tableCommand('addRowBefore') },
                    { label: 'Add column right', onSelect: () => editorRef?.tableCommand('addColumnAfter') },
                    { label: 'Add column left', onSelect: () => editorRef?.tableCommand('addColumnBefore') },
                    { label: 'Delete row', onSelect: () => editorRef?.tableCommand('deleteRow') },
                    { label: 'Delete column', onSelect: () => editorRef?.tableCommand('deleteColumn') },
                    { label: 'Delete table', danger: true, onSelect: () => editorRef?.tableCommand('deleteTable') },
                    'sep',
                  ] as const)
                : ([{ label: 'Insert table', onSelect: () => editorRef?.tableCommand('insertTable') }, 'sep'] as const)),
              { label: 'Ask Muse about selection', disabled: !sel, onSelect: () => (museSel = { text: sel, source: 'editor' }) },
              { label: 'Copy selection', disabled: !sel, onSelect: () => void navigator.clipboard.writeText(sel) },
            ])
          }}
        >
          {#key `${spaceId}-${seed}`}
            <RichEditor
              bind:this={editorRef}
              value={space.body}
              {docSearch}
              slash
              prose
              autosave
              onSave={() => void saveBody()}
              placeholder="Write an overview for this space: what lives here, how it's organized"
              fill
              class="min-w-0 flex-1"
            />
          {/key}
          <KbMuseBar
            context={`Overview document for the knowledge space “${name || space.name}”.`}
            currentText={() => editorRef?.getMarkdown() ?? space.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => (museSel = null)}
            surgical={!!museSel}
            onAccept={async (md) => {
              await save({ body: md })
              seed += 1
              museSel = null
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              if (museSel.source === 'editor') {
                editorRef?.replaceSelection(replacement)
                void saveBody()
              } else {
                const body = editorRef?.getMarkdown() ?? space.body
                await save({ body: body.replace(museSel.text, replacement) })
                seed += 1
              }
              museSel = null
            }}
          />
        </div>
      {:else}
        <div class="flex min-w-0 flex-1 flex-col">
          <div
            class="re-prose min-w-0 flex-1 overflow-y-auto"
            role="presentation"
            oncontextmenu={(e) => {
              const sel = window.getSelection()?.toString().trim() ?? ''
              spaceMenu.openMenu(e, [
                { label: 'Copy text', disabled: !sel && !space.body, onSelect: () => void navigator.clipboard.writeText(sel || space.body) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${spaceId}`) },
                ...(sel ? [{ label: 'Ask Muse about selection', onSelect: () => (museSel = { text: sel, source: 'read' as const }) }] : []),
              ])
            }}
          >
            {#if space.body.trim()}
              <Markdown class="tiptap" children={space.body} />
            {:else}
              <div class="mx-auto max-w-[46rem] px-6 py-8">
                <button type="button" onclick={() => (mode = 'edit')} class="text-sm text-muted hover:text-fg">
                  No overview yet. Click to describe this space.
                </button>
              </div>
            {/if}
          </div>
          <KbMuseBar
            context={`Overview document for the knowledge space “${space.name}”.`}
            currentText={() => space.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => (museSel = null)}
            surgical={!!museSel && space.body.includes(museSel.text)}
            onAccept={async (md) => {
              await save({ body: md })
              museSel = null
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              await save({ body: space.body.replace(museSel.text, replacement) })
              museSel = null
            }}
          />
        </div>
      {/if}
      {#if showToc}
        <KbTocPanel {headings} onJump={scrollToHeading} onClose={() => (showToc = false)} emptyText="No headings yet." />
      {/if}
      {#if showHistory}
        <div in:fly={{ x: 8, duration: 180 }} out:fade={QUICK} class="w-64 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
          <KbHistoryRail
            kind="kb-space"
            id={spaceId}
            onRestore={async (content) => {
              const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
              const nm = m ? m[1]!.trim() : name
              const b = m ? m[2]! : content
              name = nm
              await save({ name: nm, body: b })
              seed += 1
            }}
          />
        </div>
      {/if}
    </div>
    <ContextMenu menu={spaceMenu} />
    <PermissionsModal
      open={shareOpen}
      onClose={() => (shareOpen = false)}
      kind="spaces"
      id={spaceId}
      label={space.name}
      visibility={space.visibility}
      editPolicy={space.editPolicy}
      publicSlug={space.publicSlug}
      canManage={isOwner.current}
      onSave={(patch) => save(patch)}
    />
  </div>
{/if}
