<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink, History, Maximize2, Minimize2, MoreHorizontal, Paperclip, Star, Trash2, Upload } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Chip from '@/components/ui/Chip.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm, alert } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import BrainRoutingSelect from '@/components/kb/BrainRoutingSelect.svelte'
  import PermissionsModal from '@/components/kb/PermissionsModal.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage, postJsonOr } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { fade, fly, slide, GROW_X } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'
  import { deleteArtifact, saveArtifact, uploadFile, useArtifact } from '@/lib/artifacts'
  import { KIND_LABEL } from './artifacts'
  import ArtifactHistory from './ArtifactHistory.svelte'
  import ArtifactPageSkeleton from './ArtifactPageSkeleton.svelte'
  import ArtifactSheetView from './ArtifactSheetView.svelte'

  let { id, onDeleted }: { id: string; onDeleted: () => void } = $props()

  const qc = useQueryClient()
  const session = useSession()
  const me = $derived(session.data)
  const artifactQuery = useArtifact(() => id)
  const artifact = $derived(artifactQuery.data)
  let editorRef = $state<RichEditor | null>(null)
  let title = $state('')
  let saving = $state(false)
  let dirty = $state(false)
  let mode = $state<'read' | 'edit'>('read')
  let emojiOpen = $state(false)
  let fullscreen = $state(false)
  let shareOpen = $state(false)
  let showHistory = $state(false)
  let seed = $state(0)
  let html = $state('') // microsite source
  let htmlTimer: ReturnType<typeof setTimeout> | null = null
  let initMode = false
  $effect(() => {
    if (artifact) title = artifact.title
    if (artifact && !initMode) {
      initMode = true
      mode = artifact.body.trim() ? 'read' : 'edit'
      if (artifact.kind === 'microsite') html = artifact.body
    }
  })
  $effect(() => () => {
    if (htmlTimer) clearTimeout(htmlTimer)
  })

  const isOwner = $derived(!!artifact && !!me && (artifact.ownerUserId ? artifact.ownerUserId === me.id : artifact.createdBy === (me.email ?? me.name)))

  const save = async (patch: Parameters<typeof saveArtifact>[1]) => {
    saving = true
    try {
      await saveArtifact(id, patch)
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      await qc.invalidateQueries({ queryKey: ['artifacts'] })
      dirty = false
    } catch (e) {
      // dirty stays true: the editor's content is not saved, and must not
      // read as saved.
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
    } finally {
      saving = false
    }
  }
  const saveBody = () => save({ title, body: editorRef?.getMarkdown() ?? artifact?.body ?? '' })
  const editHtml = (v: string) => {
    html = v
    dirty = true
    if (htmlTimer) clearTimeout(htmlTimer)
    htmlTimer = setTimeout(() => void save({ body: v }), 700)
  }
  let uploading = $state(false)
  const onPickFile = async (file: File | null | undefined) => {
    if (!file) return
    uploading = true
    try {
      const up = await uploadFile(file)
      await save({ storageRef: up.id, contentType: up.mime, ...(!artifact?.title || artifact.title === 'Untitled' ? { title: up.filename } : {}) })
    } catch {
      /* surfaced by the empty state staying put */
    } finally {
      uploading = false
    }
  }

  let exporting = $state(false)
  const exportToGoogle = async () => {
    exporting = true
    try {
      // 409/422/502 bodies are answers this flow already branches on — the
      // not_connected confirm hand-off, and the human `message` for the alert.
      // Anything else (403, 404, network) rejects into the catch.
      const j = await postJsonOr<{ file?: { url: string }; error?: string; message?: string }>(
        `/api/artifacts/${id}/export/google`,
        undefined,
        [409, 422, 502],
      )
      if (j.file?.url) {
        await qc.invalidateQueries({ queryKey: ['artifact', id] })
        window.open(j.file.url, '_blank', 'noopener')
      } else if (j.error === 'not_connected') {
        if (await confirm({ title: 'Connect Google', message: 'Connect a Google account to export to Drive. Go to Settings now?', confirmLabel: 'Go to Settings' })) {
          window.location.href = '/settings'
        }
      } else {
        await alert({ title: 'Export failed', message: j.message ?? 'Export to Google Drive failed.' })
      }
    } catch (e) {
      await alert({ title: 'Export failed', message: errorMessage(e) })
    } finally {
      exporting = false
    }
  }
  const googleLabel = $derived(artifact?.kind === 'sheet' ? 'Export to Google Sheets' : artifact?.kind === 'file' ? 'Export to Google Drive' : 'Export to Google Docs')

  const menuItems = (): ContextMenuEntry[] => {
    if (!artifact) return []
    const entries: ContextMenuEntry[] = [
      { label: showHistory ? 'Hide history' : 'Version history', icon: [History, { size: 13 }], onSelect: () => (showHistory = !showHistory) },
      { label: exporting ? 'Exporting' : googleLabel, icon: [Upload, { size: 13 }], disabled: exporting, onSelect: () => void exportToGoogle() },
    ]
    const driveUrl = artifact.googleFileUrl
    if (driveUrl) entries.push({ label: 'Open in Google Drive', icon: [ExternalLink, { size: 13 }], onSelect: () => window.open(driveUrl, '_blank', 'noopener,noreferrer') })
    entries.push({
      label: 'Delete file',
      icon: [Trash2, { size: 13 }],
      danger: true,
      onSelect: async () => {
        if (!(await confirm({ title: 'Delete file', message: `Delete "${artifact.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        try {
          await deleteArtifact(id)
        } catch (e) {
          pushToast({ title: 'Delete failed', body: errorMessage(e), tone: 'danger' })
          return
        }
        await qc.invalidateQueries({ queryKey: ['artifacts'] })
        onDeleted()
      },
    })
    return entries
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (fullscreen && e.key === 'Escape') fullscreen = false
  }}
/>

<!-- `if (!artifact)` covered all three answers, so a 500 and a deleted artifact
     both left fourteen skeleton nodes shimmering — measured unchanged at +53s
     in a browser, with no error, no retry, and no way to tell which it was.
     `useArtifact` resolves `null` only for a real 404 and rejects otherwise. -->
{#if artifactQuery.isError && artifact === undefined}
  <div in:fade={{ duration: 150 }}>
    <QueryError
      error={artifactQuery.error}
      title="Could not load this document"
      onRetry={() => void artifactQuery.refetch()}
    />
  </div>
{:else if artifact === null}
  <div in:fade={{ duration: 150 }}>
    <EmptyState icon="⧉" title="Document not found" hint="It may have been deleted, or it is no longer shared with you." />
  </div>
{:else if !artifact}
  <!-- Kind is unknown until the fetch lands, so use the doc-page shape (toolbar
       + centered prose bars) as the default stand-in for every kind. -->
  <ArtifactPageSkeleton />
{:else}
  <div class={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'h-full')}>
    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
      <div class="relative shrink-0">
        <button type="button" onclick={() => (emojiOpen = !emojiOpen)} class="rounded-md px-1 text-xl leading-none transition-colors dither-fill" title="Set icon">
          {artifact.icon ?? '📄'}
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
          value={title}
          oninput={(e) => {
            title = e.currentTarget.value
            dirty = true
          }}
          onblur={() => dirty && void saveBody()}
          onkeydown={inlineEditKeys(() => artifact && (title = artifact.title))}
          class="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
          placeholder="Untitled"
        />
      {:else}
        <h1 class="min-w-0 flex-1 truncate font-sans text-lg font-semibold text-fg">{artifact.title}</h1>
      {/if}
      <Chip>{KIND_LABEL[artifact.kind]}</Chip>
      <Segmented
        size="xs"
        options={[{ id: 'read', label: 'Read' }, { id: 'edit', label: 'Edit' }] as const}
        value={mode}
        onChange={(m) => {
          if (m === 'read' && mode === 'edit') void saveBody()
          mode = m
        }}
      />
      <Button variant="outline" size="sm" class="shrink-0 capitalize" title="Share &amp; permissions" onclick={() => (shareOpen = true)}>
        {artifact.visibility}
      </Button>
      {#if isOwner}
        <Button
          variant={artifact.official ? 'primary' : 'outline'}
          size="sm"
          class="shrink-0"
          title="Official files are mirrored into the knowledgebase and ground the org brain"
          onclick={() => void save({ official: !artifact.official })}
        >
          <Star size={13} class="mr-1" /> {artifact.official ? 'Official' : 'Make official'}
        </Button>
      {/if}
      <BrainRoutingSelect value={artifact.ragRouting} canEdit={isOwner} onChange={(ragRouting) => void save({ ragRouting })} />
      <Button variant="ghost" size="sm" class="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onclick={() => (fullscreen = !fullscreen)}>
        {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </Button>
      <DropdownMenu class="shrink-0" items={menuItems}>
        {#snippet trigger(_open: boolean)}
          <Button variant="ghost" size="sm" title="More">
            <MoreHorizontal size={14} />
          </Button>
        {/snippet}
      </DropdownMenu>
    </div>

    <div class="flex min-h-0 flex-1">
      {#if artifact.kind === 'doc'}
        {#if mode === 'edit'}
          {#key `${id}-${seed}`}
            <RichEditor bind:this={editorRef} value={artifact.body} slash prose autosave onSave={() => void saveBody()} placeholder="Start writing" fill class="min-w-0 flex-1" />
          {/key}
        {:else}
          <!-- Tab-pane grammar on the READ pane only: the edit pane holds live
               editor state (and is seed-keyed for Muse/revision swaps), so it
               keeps its hard cut rather than replaying an entrance. -->
          <div in:fly={{ y: 6, duration: 200 }} class="re-prose min-w-0 flex-1 overflow-y-auto">
            {#if artifact.body.trim()}
              <Markdown class="tiptap" children={artifact.body} />
            {:else}
              <div class="mx-auto max-w-[46rem] px-6 py-8">
                <button type="button" onclick={() => (mode = 'edit')} class="text-sm text-muted hover:text-fg">
                  Empty document. Click to start.
                </button>
              </div>
            {/if}
          </div>
        {/if}
      {:else if artifact.kind === 'microsite'}
        {#if mode === 'edit'}
          <Textarea
            value={html}
            oninput={(e) => editHtml(e.currentTarget.value)}
            onblur={() => dirty && void save({ body: html })}
            spellcheck={false}
            placeholder={'<!doctype html>\n<html>'}
            class="min-w-0 flex-1 rounded-none border-0 font-mono text-xs leading-relaxed"
          />
        {:else if artifact.body.trim()}
          <!-- Sandboxed: scripts run, but no same-origin — can't touch the app. -->
          <iframe in:fly={{ y: 6, duration: 200 }} title={artifact.title} srcdoc={artifact.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" class="min-w-0 flex-1 border-0 bg-white"></iframe>
        {:else}
          <div in:fly={{ y: 6, duration: 200 }} class="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
            <button type="button" onclick={() => (mode = 'edit')} class="hover:text-fg">Empty microsite. Switch to Edit to write HTML.</button>
          </div>
        {/if}
      {:else if artifact.kind === 'sheet'}
        {#key `${id}-${seed}`}
          <ArtifactSheetView value={artifact.body} editable={mode === 'edit'} onSave={(body) => void save({ body })} />
        {/key}
      {:else if artifact.kind === 'file'}
        <div class="min-w-0 flex-1 overflow-y-auto p-8">
          {#if artifact.storageRef}
            <div class="mx-auto max-w-[var(--read-width)]">
              {#if artifact.contentType?.startsWith('image/')}
                <img src={`/api/uploads/${artifact.storageRef}`} alt={artifact.title} class="mb-4 max-h-[60vh] rounded-lg border border-line" />
              {:else}
                <div class="mb-4 flex items-center gap-3 rounded-lg border border-line bg-panel p-4">
                  <Paperclip size={20} class="shrink-0 text-muted" />
                  <div class="min-w-0 flex-1">
                    <div class="truncate font-sans text-sm text-fg">{artifact.title}</div>
                    <div class="font-mono text-[11px] text-muted">{artifact.contentType ?? 'file'}</div>
                  </div>
                </div>
              {/if}
              <div class="flex gap-2">
                <a href={`/api/uploads/${artifact.storageRef}`} target="_blank" rel="noreferrer" class={buttonClasses({ size: 'sm' })}>
                  Download
                </a>
                {#if isOwner}
                  <label class={cn(buttonClasses({ size: 'sm', variant: 'outline' }), 'cursor-pointer')}>
                    <input type="file" class="hidden" onchange={(e) => void onPickFile(e.currentTarget.files?.[0])} />
                    Replace file
                  </label>
                {/if}
              </div>
            </div>
          {:else}
            <label class="mx-auto flex max-w-lg cursor-pointer flex-col items-center gap-2 rounded-lg border border-dashed border-line p-12 text-center transition-colors hover:border-[var(--theme-accent-border)]">
              <input type="file" class="hidden" onchange={(e) => void onPickFile(e.currentTarget.files?.[0])} />
              <Paperclip size={22} class="text-muted" />
              <div class="text-sm text-fg">{uploading ? 'Uploading' : 'Click to upload a file'}</div>
              <div class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Up to 25 MB · stored and hosted by Talaria</div>
            </label>
          {/if}
        </div>
      {:else}
        <div class="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
          {KIND_LABEL[artifact.kind]} files are coming soon.
        </div>
      {/if}
      {#if showHistory}
        <!-- IN-FLOW rail: slide={GROW_X} on both legs (ANIMATIONS.md); inner
             wrapper pinned to the resting width so rows clip, not rewrap. -->
        <div transition:slide={GROW_X} class="shrink-0 overflow-y-auto border-l border-line-subtle">
        <div class="w-64 p-3">
          <ArtifactHistory
            {id}
            onRestore={async (content) => {
              const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
              const t = m ? m[1]!.trim() : title
              const b = m ? m[2]! : content
              title = t
              await save({ title: t, body: b })
              seed += 1
            }}
          />
        </div>
        </div>
      {/if}
    </div>

    <!-- Footer status line — mono chrome voice with right-aligned meta (§8). -->
    <div class="flex items-center gap-2 border-t border-line-subtle px-6 py-2 font-mono text-[10px] tracking-[0.05em] text-muted">
      <span>edited {relativeTime(artifact.updatedAt)}{artifact.updatedBy ? ` by ${artifact.updatedBy}` : ''}</span>
      <span class="ml-auto"></span>
      {#if mode === 'edit' && artifact.kind !== 'file'}<span class="uppercase">{saving ? 'Saving' : 'Saved'}</span>{/if}
    </div>

    <PermissionsModal
      open={shareOpen}
      onClose={() => (shareOpen = false)}
      kind="artifacts"
      {id}
      label={artifact.title}
      visibility={artifact.visibility}
      editPolicy={artifact.editPolicy}
      publicSlug={artifact.publicSlug}
      canManage={isOwner}
      onSave={(patch) => save(patch)}
    />
  </div>
{/if}
