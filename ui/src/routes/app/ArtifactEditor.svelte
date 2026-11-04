<script lang="ts" module>
  import type { Snippet } from 'svelte'

  /** The pieces of the toolbar a host lays out in its own header row. */
  export interface PaneParts {
    /** The title — plain in Read, an input in Edit (renaming stays here). */
    title: Snippet
    /** The Read/Edit switch. */
    tools: Snippet
    /** The More menu, with the share and official controls folded into it. */
    menu: Snippet
  }
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Download, ExternalLink, History, Maximize2, Minimize2, MoreHorizontal, Paperclip, Share2, Star, Trash2, Upload } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Chip from '@/components/ui/Chip.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import RecordTitle from '@/components/ui/RecordTitle.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { confirm, alert } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import BrainRoutingSelect from '@/components/kb/BrainRoutingSelect.svelte'
  import PermissionsModal from '@/components/kb/PermissionsModal.svelte'
  import { cn } from '@/lib/cn'
  import { downloadFile } from '@/lib/download-file'
  import { errorMessage, postJson, postJsonOr } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { fade, fly, slide, GROW_X } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'
  import { deleteArtifact, officeExport, officeExportHref, putArtifact, uploadFile, useArtifact, type Artifact, type ArtifactPatch } from '@/lib/artifacts'
  import { followRevisions, historyKey } from '@/lib/revisions.svelte'
  import { guardedSaver } from '@/lib/save-base.svelte'
  import { KIND_LABEL } from './artifacts'
  import FilePreview from './FilePreview.svelte'
  import KbHistoryRail from './KbHistoryRail.svelte'
  import KbDocPageSkeleton from './KbDocPageSkeleton.svelte'
  import ArtifactSheetView from './ArtifactSheetView.svelte'

  let {
    id,
    onDeleted,
    header,
    override,
  }: {
    id: string
    onDeleted: () => void
    /** PANE MODE. A host that draws one header row for every kind of file (the
     *  work pane: type, title, Comments, Open, fullscreen, Close) renders this
     *  editor's own controls INTO that row rather than stacking a second
     *  toolbar under it. Called with null until the artifact has loaded, so the
     *  row stands from the first paint. In pane mode the host owns fullscreen,
     *  the brain-routing select stays on the Files page, and the share and
     *  official buttons move into the More menu — a 44% column cannot hold the
     *  page toolbar on one line. */
    header?: Snippet<[PaneParts | null]>
    /** Pane mode: rendered INSTEAD of the body (the host's comments), between
     *  the header row and the footer, so the editor's mode and title survive
     *  the swap. */
    override?: Snippet
  } = $props()

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
  let fullscreen = $state(false)
  let shareOpen = $state(false)
  let showHistory = $state(false)
  let seed = $state(0)
  let html = $state('') // microsite source
  let htmlTimer: ReturnType<typeof setTimeout> | null = null
  // One save door, one save at a time; the editing surface's buffer saves
  // GUARDED — it names the version it was seeded from, and an agent's
  // update_document (or another person's save) landing meanwhile refuses it
  // instead of being wiped by it: `saver.conflict`, the inline line above the
  // body, at that save and never on open (lib/save-base.svelte.ts).
  const saver = guardedSaver((p: ArtifactPatch & { baseUpdatedAt?: string }) => putArtifact(id, p))
  let initMode = false
  $effect(() => {
    if (artifact) title = artifact.title
    if (artifact && !initMode) {
      initMode = true
      mode = artifact.body.trim() ? 'read' : 'edit'
      if (artifact.kind === 'microsite') html = artifact.body
      saver.rebase(artifact)
    }
  })

  // READ PANES FOLLOW THE SERVER: a document or page in read mode refreshes
  // when a revision lands (an agent's update_document / edit_document,
  // another person's save). No notice — the Version history rail says who
  // changed what. Edit mode never follows; a sheet's grid is seeded once.
  followRevisions(
    () => 'artifact',
    () => id,
    {
      active: () => mode === 'read' && (artifact?.kind === 'doc' || artifact?.kind === 'microsite'),
      onMoved: () => void qc.invalidateQueries({ queryKey: ['artifact', id] }),
    },
  )
  $effect(() => () => {
    if (htmlTimer) clearTimeout(htmlTimer)
  })

  const isOwner = $derived(!!artifact && !!me && (artifact.ownerUserId ? artifact.ownerUserId === me.id : artifact.createdBy === (me.email ?? me.name)))

  /** Resolves true when the save landed. `guard`: the edit surface's buffer. */
  const save = async (patch: ArtifactPatch, opts: { guard?: boolean } = {}): Promise<boolean> => {
    saving = true
    try {
      const saved = await saver.save(patch, opts)
      void qc.invalidateQueries({ queryKey: historyKey('artifact', id) })
      // A conflict: the line is up, and dirty stays — nothing was saved.
      if (!saved) return false
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      await qc.invalidateQueries({ queryKey: ['artifacts'] })
      dirty = false
      return true
    } catch (e) {
      // dirty stays true: the editor's content is not saved, and must not
      // read as saved.
      toastError('Save failed', e)
      return false
    } finally {
      saving = false
    }
  }
  /** The text the editing surface holds — a document's editor, a page's
   *  HTML. A sheet saves its own grid; its title saves alone. */
  const bufferText = (): string | undefined =>
    artifact?.kind === 'doc' ? (editorRef?.getMarkdown() ?? artifact.body) : artifact?.kind === 'microsite' ? html : undefined
  const saveBody = () => {
    const body = bufferText()
    return save(body === undefined ? { title } : { title, body }, { guard: mode === 'edit' })
  }
  /** The conflict line's override: the buffer over the newer version (which
   *  stays in version history). A sheet re-saves its grid on its next edit. */
  const keepMine = () => {
    const body = bufferText()
    return save(body === undefined ? { title } : { title, body })
  }
  /** Read ⇄ Edit. Entering edit seeds a document's editor (and a page's HTML)
   *  from the version on screen, so that is the base; a sheet's grid was
   *  seeded when it mounted, so its base stays. Leaving edit saves — unless
   *  the last save was refused, when leaving would drop the unsaved edits. */
  const setMode = async (m: 'read' | 'edit') => {
    if (m === mode || !artifact) return
    if (m === 'edit') {
      if (artifact.kind === 'microsite') html = artifact.body
      if (artifact.kind !== 'sheet') saver.rebase(artifact)
    } else if (saver.conflict) {
      const discard = await confirm({
        title: 'Discard your unsaved edits?',
        message: 'This document changed while you were editing, so your edits were not saved. Discard them and read the current version, or stay and keep yours.',
        confirmLabel: 'Discard',
        danger: true,
      })
      if (!discard) return
      saver.rebase(artifact)
      void qc.invalidateQueries({ queryKey: ['artifact', id] })
    } else {
      void saveBody()
    }
    mode = m
  }
  const editHtml = (v: string) => {
    html = v
    dirty = true
    if (htmlTimer) clearTimeout(htmlTimer)
    htmlTimer = setTimeout(() => void save({ body: v }, { guard: true }), 700)
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
      // 409/400/502 bodies are answers this flow already branches on — the
      // not_connected confirm hand-off, and the human `message` for the alert
      // (400 = not_exportable, the artifact-kind refusal). Anything else
      // (403, 404, network) rejects into the catch.
      const j = await postJsonOr<{ file?: { url: string }; error?: string; message?: string }>(
        `/api/artifacts/${id}/export/google`,
        undefined,
        [409, 400, 502],
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
  /** THE SAME EXPORT WITHOUT GOOGLE: a document becomes .docx, a sheet .xlsx,
   *  built in process and answered as a download — no connected account and
   *  nothing left behind. A `file` artifact gets nothing here, because its own
   *  bytes already download through /api/uploads. */
  const downloadOffice = async () => {
    const spec = officeExport(artifact?.kind)
    if (!artifact || !spec) return
    try {
      await downloadFile(officeExportHref(id, spec.format), `${artifact.title || 'download'}.${spec.format}`)
    } catch (e) {
      toastError('Could not download', e)
    }
  }
  let pulling = $state(false)
  const pullFromGoogle = async () => {
    if (!(await confirm({
      title: 'Pull from Google',
      message: 'Replace the Talaria copy with the Google Doc? Edits here that have not been synced will be overwritten.',
      confirmLabel: 'Pull and overwrite',
      danger: true,
    }))) return
    pulling = true
    try {
      await postJson(`/api/artifacts/${id}/pull/google`)
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
    } catch (e) {
      await alert({ title: 'Pull failed', message: errorMessage(e) })
    } finally {
      pulling = false
    }
  }
  const googleLabel = $derived(artifact?.googleFileUrl ? 'Sync to Google' : artifact?.kind === 'sheet' ? 'Export to Google Sheets' : artifact?.kind === 'file' ? 'Export to Google Drive' : 'Export to Google Docs')

  const menuItems = (): ContextMenuEntry[] => {
    if (!artifact) return []
    // In pane mode the toolbar's share and official buttons live here instead.
    const paneEntries: ContextMenuEntry[] = header
      ? [
          { label: `Share & permissions (${artifact.visibility})`, icon: [Share2, { size: 13 }], onSelect: () => (shareOpen = true) },
          ...(isOwner
            ? [{ label: artifact.official ? 'Remove official' : 'Make official', icon: [Star, { size: 13 }], onSelect: () => void save({ official: !artifact.official }) } as ContextMenuEntry]
            : []),
          'sep',
        ]
      : []
    const entries: ContextMenuEntry[] = [
      ...paneEntries,
      { label: showHistory ? 'Hide history' : 'Version history', icon: [History, { size: 13 }], onSelect: () => (showHistory = !showHistory) },
      { label: exporting ? 'Exporting' : googleLabel, icon: [Upload, { size: 13 }], disabled: exporting, onSelect: () => void exportToGoogle() },
    ]
    // Beside the Google export, the offline twin of it.
    const office = officeExport(artifact.kind)
    if (office) {
      entries.push({ label: `Download as ${office.label}`, icon: [Download, { size: 13 }], onSelect: () => void downloadOffice() })
    }
    const driveUrl = artifact.googleFileUrl
    if (driveUrl) {
      entries.push({ label: 'Open in Google', icon: [ExternalLink, { size: 13 }], onSelect: () => window.open(driveUrl, '_blank', 'noopener,noreferrer') })
      entries.push({ label: pulling ? 'Pulling' : 'Pull from Google', icon: [Upload, { size: 13 }], disabled: pulling, onSelect: () => void pullFromGoogle() })
    }
    entries.push({
      label: 'Delete file',
      icon: [Trash2, { size: 13 }],
      danger: true,
      onSelect: async () => {
        if (!(await confirm({ title: 'Delete file', message: `Delete "${artifact.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        try {
          await deleteArtifact(id)
        } catch (e) {
          toastError('Delete failed', e)
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

{#snippet paneTitle()}
  {#if artifact}
    {#if mode === 'edit'}
      <Input
        value={title}
        placeholder="Untitled"
        aria-label="Title"
        oninput={(e) => {
          title = e.currentTarget.value
          dirty = true
        }}
        onblur={() => dirty && void saveBody()}
        onkeydown={inlineEditKeys(() => artifact && (title = artifact.title))}
        class="h-7 min-w-0 flex-1 border-0 bg-transparent px-0 font-sans text-sm font-medium focus:border-0"
      />
    {:else}
      <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg" title={artifact.title}>
        {artifact.title || 'Untitled document'}
      </span>
    {/if}
  {/if}
{/snippet}

{#snippet paneTools()}
  <!-- A file upload has nothing to edit in place, so it has no mode either. -->
  {#if artifact && artifact.kind !== 'file'}
    <Segmented
      size="xs"
      options={[{ id: 'read', label: 'Read' }, { id: 'edit', label: 'Edit' }] as const}
      value={mode}
      onChange={(m) => void setMode(m)}
    />
  {/if}
{/snippet}

{#snippet paneMenu()}
  <DropdownMenu class="shrink-0" items={menuItems}>
    {#snippet trigger(_open: boolean)}
      <IconButton size="sm" title="More">
        <MoreHorizontal size={14} />
      </IconButton>
    {/snippet}
  </DropdownMenu>
{/snippet}

{#if header}
  <!-- PANE MODE: one column, the host's header row on top, then the body, then
       the footer. `flex-1` rather than `h-full`: the host column is a flex
       column, and that is what bounds the body's own scroll box. -->
  <div class="flex min-h-0 min-w-0 flex-1 flex-col">
    {@render header(artifact ? { title: paneTitle, tools: paneTools, menu: paneMenu } : null)}
    {#if artifactQuery.isError && artifact === undefined}
      <div in:fade={{ duration: 150 }} class="min-h-0 flex-1 overflow-y-auto">
        <QueryError
          error={artifactQuery.error}
          title="Could not load this document"
          onRetry={() => void artifactQuery.refetch()}
        />
      </div>
    {:else if artifact === null}
      <div in:fade={{ duration: 150 }} class="min-h-0 flex-1">
        <EmptyState icon="⧉" title="Document not found" hint="It may have been deleted, or it is no longer shared with you." />
      </div>
    {:else if !artifact}
      <!-- The prose half of the page skeleton: the row above is already the
           real header, so a skeleton toolbar under it would be a second one. -->
      <div aria-hidden="true" class="min-h-0 flex-1 space-y-3.5 overflow-hidden px-6 py-8">
        {#each ['88%', '96%', '72%', '91%', '84%', '60%'] as w, i (i)}
          <div style:width={w}><Skeleton class="h-3 w-full rounded-full" /></div>
        {/each}
      </div>
    {:else}
      {#if override}
        <div class="flex min-h-0 flex-1">{@render override()}</div>
      {:else}
        {@render conflictRow()}
        {@render bodyRow(artifact)}
      {/if}
      {@render footerRow(artifact)}
      {@render shareModal(artifact)}
    {/if}
  </div>
{:else}
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
  <!-- The knowledge editors' skeleton, not a second copy of it: the two
      layouts are the same (toolbar over a centred prose column), and only the
      prose widths had drifted — 58% where knowledge's list lands 88% on the
      tenth bar. Ten bars is the artifact page's shape. -->
  <KbDocPageSkeleton bars={10} />
{:else}
  <div class={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'h-full')}>
    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
      <!-- Read mode shows the record, edit mode the buffer — the artifact's own
           save path and dirty flag stay here. -->
      <RecordTitle
        icon={artifact.icon}
        onIconPick={(e) => void save({ icon: e })}
        onIconClear={() => void save({ icon: null })}
        value={mode === 'edit' ? title : artifact.title}
        onInput={(v) => {
          title = v
          dirty = true
        }}
        onCommit={() => dirty && void saveBody()}
        onCancel={() => artifact && (title = artifact.title)}
        editing={mode === 'edit'}
      />
      <Chip>{KIND_LABEL[artifact.kind]}</Chip>
      <Segmented
        size="xs"
        options={[{ id: 'read', label: 'Read' }, { id: 'edit', label: 'Edit' }] as const}
        value={mode}
        onChange={(m) => void setMode(m)}
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

    {@render conflictRow()}
    {@render bodyRow(artifact)}

    {@render footerRow(artifact)}

    {@render shareModal(artifact)}
  </div>
{/if}
{/if}

{#snippet conflictRow()}
  {#if saver.conflict}
    <div
      transition:slide={{ duration: 150 }}
      class="flex shrink-0 items-center gap-2 border-b border-line-subtle px-6 py-1.5 text-xs"
      role="alert"
      data-save-conflict
    >
      <span class="min-w-0 flex-1 truncate text-warning">Not saved: this document changed while you were editing.</span>
      <Button variant="outline" size="xs" class="shrink-0" onclick={() => (showHistory = true)}>Review in version history</Button>
      <Button variant="ghost" size="xs" class="shrink-0" onclick={() => void keepMine()}>Keep mine</Button>
    </div>
  {/if}
{/snippet}

{#snippet bodyRow(artifact: Artifact)}
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
              <button type="button" onclick={() => void setMode('edit')} class="text-sm text-muted hover:text-fg">
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
          onblur={() => dirty && void save({ body: html }, { guard: true })}
          spellcheck={false}
          placeholder={'<!doctype html>\n<html>'}
          class="min-w-0 flex-1 rounded-none border-0 font-mono text-xs leading-relaxed"
        />
      {:else if artifact.body.trim()}
        <!-- Sandboxed: scripts run, but no same-origin — can't touch the app. -->
        <iframe in:fly={{ y: 6, duration: 200 }} title={artifact.title} srcdoc={artifact.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" class="min-w-0 flex-1 border-0 bg-white"></iframe>
      {:else}
        <div in:fly={{ y: 6, duration: 200 }} class="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
          <button type="button" onclick={() => void setMode('edit')} class="hover:text-fg">Empty microsite. Switch to Edit to write HTML.</button>
        </div>
      {/if}
    {:else if artifact.kind === 'sheet'}
      {#key `${id}-${seed}`}
        <ArtifactSheetView value={artifact.body} editable={mode === 'edit'} onSave={(body) => void save({ body }, { guard: true })} />
      {/key}
    {:else if artifact.kind === 'file'}
      <div class="min-w-0 flex-1 overflow-y-auto p-8">
        {#if artifact.storageRef}
          <div class="mx-auto max-w-[var(--read-width)]">
            <div class="mb-4">
              <FilePreview
                url={`/api/uploads/${artifact.storageRef}`}
                title={artifact.title}
                contentType={artifact.contentType}
              />
            </div>
            <div class="flex gap-2">
              <Button
                size="sm"
                onclick={() =>
                  void downloadFile(`/api/uploads/${artifact.storageRef}`, artifact.title).catch((e) =>
                    toastError('Could not download', e),
                  )}
              >
                Download
              </Button>
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
        <KbHistoryRail
          kind="artifact"
          {id}
          format={artifact.kind === 'doc' ? 'markdown' : 'text'}
          onRestore={async (content) => {
            const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
            const t = m ? m[1]!.trim() : title
            const b = m ? m[2]! : content
            title = t
            // A deliberate replace: unguarded, and the surfaces reseed from it.
            if (!(await save({ title: t, body: b }))) return
            if (artifact.kind === 'microsite') html = b
            seed += 1
          }}
        />
      </div>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet footerRow(artifact: Artifact)}
  <!-- Footer status line — mono chrome voice with right-aligned meta (§8). -->
  <div class="flex items-center gap-2 border-t border-line-subtle px-6 py-2 font-mono text-[10px] tracking-[0.05em] text-muted">
    <span>edited {relativeTime(artifact.updatedAt)}{artifact.updatedBy ? ` by ${artifact.updatedBy}` : ''}</span>
    <span class="ml-auto"></span>
    {#if mode === 'edit' && artifact.kind !== 'file'}<span class="uppercase">{saving ? 'Saving' : saver.conflict ? 'Not saved' : 'Saved'}</span>{/if}
  </div>
{/snippet}

{#snippet shareModal(artifact: Artifact)}
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
    onSave={async (patch) => {
      await save(patch)
    }}
  />
{/snippet}
