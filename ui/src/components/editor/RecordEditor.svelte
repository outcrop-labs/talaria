<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Check, History, Sparkles, Trash2, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import SendButton from '@/components/chat/SendButton.svelte'
  import StopButton from '@/components/chat/StopButton.svelte'
  import InternalEditor from './InternalEditor.svelte'
  import type { InternalEditorHandle } from './internal-editor'
  import DiffView from '@/components/fleet/DiffView.svelte'
  import { diffLines, type DiffLine } from '@/components/fleet/line-diff'
  import { fly, slide } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import type { MuseKind } from '@/lib/muse.svelte'

  type ChatMsg = { role: 'user' | 'assistant'; content: string }

  /** THE SHARED RECORD SURFACE — one editing surface per view: the form title,
   *  its description, the record's inputs and the document workbench in ONE
   *  independently scrollable main region; the record's menu (Muse, delete,
   *  unsaved, Cancel/Save) pinned to the bottom of the view, always in view.
   *  History is one icon on the title line. ONE Muse — a toggleable composer
   *  above the menu that drafts the COMPLETE record and fills the fields on
   *  Accept. Nothing is written until the record's own Save.
   *
   *  ONE MUSE PER RECORD. A record with a structured half (a skill, a
   *  template) gets its Muse HERE and must not also pass `doc.muse`: two
   *  composers on one surface is two answers to "what does this button draft
   *  for me", and the whole-form draft already carries the document. `doc.muse`
   * is for a bare document (a memory file) with no structured half. */
  let {
    kind,
    title,
    meta,
    subtitle,
    fields,
    fieldsDirty = false,
    doc,
    formMuse,
    onClose,
    onCancel,
    showHistory = $bindable(false),
    onDelete,
    class: className,
  }: {
    /** The record's kind, as a chip: 'skill', 'template', 'memory' … */
    kind: string
    /** The record's name/title, shown in the header row. */
    title: string
    /** Small right-aligned text: relative time, an owner, … */
    meta?: string
    /** A line under the header: the record's blurb. */
    subtitle?: string
    /** The record's non-document fields (its own bound inputs). Omit for a
     *  bare document. */
    fields?: Snippet<[{ dirty: boolean }]>
    /** True when a field the caller owns differs from the saved record; joins
     *  the editor's dirty flag for the record's one Save. */
    fieldsDirty?: boolean
    /** The document: what to seed, who to save to, its rail and its MUSE. */
    doc: {
      value: string
      editable?: boolean
      saving?: boolean
      /** Writes the document (the record's Save; the caller merges its fields). */
      onSave: (markdown: string) => Promise<void> | void
      /** Query params for /api/history. Omit to hide the rail. */
      history?: Record<string, string>
      /** The doc's own streaming Muse — for BARE documents (memory) only. A
       *  record with `formMuse` drafts its document through the whole-form
       *  Muse instead; see the ONE MUSE rule above. */
      muse?: { kind: MuseKind; context?: string }
    }
    /** The whole-form Muse: drafts the COMPLETE record from an instruction and
     *  fills the fields on Accept. Omit for records with no structured half. */
    formMuse?: {
      /** 'skill' / 'template' — appears in the bar's copy. */
      label: string
      /** The record, with the document at its current live text. */
      current: (docText: string) => unknown
      /** POST the instruction; resolves a complete record or `{ error }`. */
      draft: (input: { instruction: string; current: string; chat: ChatMsg[] }, signal: AbortSignal) => Promise<any>
      /** The drafted record's non-document fields, as chips. Omit for one-field records. */
      fields?: (d: any, cur: unknown) => Array<{ label: string; value: string }>
      /** The drafted record's document text — staged in the editor on Accept. */
      docOf: (d: any) => string
      /** Stage the draft in the caller's field state (never write). */
      apply: (d: any) => void
    }
    /** A control left of the record's footer buttons (a delete button). */
    footerExtra?: Snippet
    /** The surface goes away — a modal's Close, a panel back to its library.
     *  Omit where the view IS the record (a route), and the Close button is
     *  not rendered. */
    onClose?: () => void
    /** Discard the local edits: the record's fields reset to the saved record
     *  and the document restores. Omit where nothing local is worth discarding. */
    onCancel?: () => void
    /** The history rail's open state, bound for views that mirror it. */
    showHistory?: boolean
    /** Delete the open record — a trash icon right-aligned on the title line,
     *  beside the history toggle. The DOUBLE OPT-IN (typing the record's name)
     *  belongs to the caller's `confirmDelete`. Omit where there is nothing to
     *  delete (a memory file). */
    onDelete?: () => void
    class?: string
  } = $props()

  let handle = $state<InternalEditorHandle | null>(null)
  let docDirty = $state(false)
  let busy = $state(false)
  let failure = $state<unknown>(null)

  // ── whole-form Muse: instruction → complete record proposal → fields ─────
  let museOpen = $state(false)
  let instruction = $state('')
  let generating = $state(false)
  let proposal = $state<any | null>(null)
  let proposalDiff = $state<DiffLine[] | null>(null)
  let chat = $state<ChatMsg[]>([])
  let museError = $state<string | null>(null)
  let abortCtl: AbortController | null = null
  $effect(() => () => abortCtl?.abort())

  const docText = () => handle?.getMarkdown() ?? doc.value
  const dirty = $derived(fieldsDirty || docDirty)

  // THE PREVIEW SURVIVES UNTIL IT IS ACCEPTED. `apply` only fills the fields;
  // nothing is written until the record's Save — so a refused write leaves the
  // proposal on screen, exactly the way the ticket patch preview behaves.
  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || !formMuse || generating) return
    generating = true
    museError = null
    const ac = new AbortController()
    abortCtl = ac
    try {
      const d = await formMuse.draft(
        { instruction: instr, current: JSON.stringify(formMuse.current(docText())), chat },
        ac.signal,
      )
      chat = [...chat.slice(-10), { role: 'user', content: instr }, { role: 'assistant', content: JSON.stringify(d) }]
      if (d?.error !== undefined && d?.error !== '') throw new Error(d.error)
      proposal = d
      proposalDiff = diffLines(docText(), formMuse.docOf(d))
      instruction = ''
    } catch (e) {
      if (!ac.signal.aborted) museError = (e as Error).message
    } finally {
      generating = false
    }
  }

  const accept = () => {
    if (!proposal || !formMuse) return
    formMuse.apply(proposal)
    handle?.setDoc(formMuse.docOf(proposal))
    proposal = null
  }

  const cancel = () => {
    handle?.restoreDoc(doc.value)
    docDirty = false
    proposal = null
    museError = null
    onCancel?.()
  }

  const saveAll = async () => {
    busy = true
    failure = null
    try {
      await doc.onSave(docText())
      docDirty = false
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- THE SURFACE IS EXACTLY ITS CONTAINER'S HEIGHT — h-full, NO FLOOR. A floor
     taller than the pane (32rem vs a 26rem pane on a normal laptop window) is
     precisely how the menu stopped pinning: the root overflowed the pane's
     scroll wrapper and rode below the fold. Short panes are handled instead by
     the main region's internal scroll — everything the record has scrolls in
     there, however little room is left — so the root never needs to demand
     more height than the view gives it. -->
<div
  class={cn('flex h-full flex-col', className)}
  onkeydown={(e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 's') {
      e.preventDefault()
      if (dirty && !busy) void saveAll()
    }
  }}
>
  <!-- THE MAIN EDITOR PART — ONE independently scrollable container: the form
       title, its description, the inputs and the document workbench scroll
       together, exactly like a page. The menu below never moves. PADDING LIVES
       HERE, inside the scroll — the wrapper adds none, so the pinned menu runs
       FLUSH to the view's edges and its border spans the full width. -->
  <div class="relative min-h-0 flex-1">
    <div class="h-full overflow-y-auto px-4 py-4">
      <div class="mb-3 flex items-center gap-2">
        <Chip class="shrink-0">{kind}</Chip>
        <div class="min-w-0 truncate text-sm font-semibold text-fg">{title}</div>
        {#if meta}<span class="shrink-0 font-mono text-[11px] text-muted">{meta}</span>{/if}
        <!-- The record's icon cluster, right-aligned on its own title line —
             within the editor view, not on any sidebar. Delete is a trash icon
             (the typed challenge says exactly what is going); history opens the
             workbench's rail. -->
        {#if onDelete || doc.history}
          <!-- Icon TILES, not padded buttons: two icon controls side by side
               read as one cluster only when their glyphs are near each other. -->
          <div class="ml-auto flex shrink-0 items-center">
            {#if onDelete}
              <IconButton title={`Delete ${title}`} danger onclick={onDelete}>
                <Trash2 size={14} />
              </IconButton>
            {/if}
            {#if doc.history}
              <IconButton title="Version history" active={showHistory} onclick={() => (showHistory = !showHistory)}>
                <History size={14} />
              </IconButton>
            {/if}
          </div>
        {/if}
      </div>
      {#if subtitle}<p class="mb-3 -mt-1.5 text-xs text-muted">{subtitle}</p>{/if}

      {#if fields}{@render fields({ dirty })}{/if}

      <div class="h-[60vh] min-h-[20rem]">
        <InternalEditor
          bind:this={handle}
          bind:dirty={docDirty}
          bind:showHistory
          value={doc.value}
          editable={doc.editable}
          saving={doc.saving ?? busy}
          onSave={doc.onSave}
          history={doc.history}
          muse={doc.muse}
          fill
          withActions={false}
        />
      </div>
    </div>

    {#if proposal && formMuse}
      <!-- The proposal REVIEW covers the whole editor part — fields and
           document, which is what the draft proposes to fill. The editor stays
           mounted underneath (unsaved edits and all — Discard hands them
           back), the surface does not grow when a draft arrives, and the
           pinned menu stays where it was. Tab-pane grammar: the card rises in
           and cuts out hard. -->
      <div in:fly={{ y: 6, duration: 200 }} class="absolute inset-0 z-10 flex flex-col rounded-lg border border-accent/30 bg-panel p-3">
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="mr-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{formMuse.label}</span>
          {#each formMuse.fields?.(proposal, formMuse.current(docText())) ?? [] as f (f.label)}
            <span class="rounded-full border border-accent/40 bg-accent/10 px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-fg">
              {f.label} → {f.value}
            </span>
          {/each}
        </div>
        <div class="mb-1 mt-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Document</div>
        <div class="min-h-0 flex-1 overflow-y-auto">
          <DiffView diff={proposalDiff} fallback={formMuse.docOf(proposal)} />
        </div>
        <div class="flex justify-end gap-2 pt-2">
          <Button size="sm" variant="outline" onclick={() => (proposal = null)}>
            <X size={13} /> Discard
          </Button>
          <Button size="sm" onclick={accept}>
            <Check size={13} /> Fill the form
          </Button>
        </div>
      </div>
    {/if}
  </div>

  {#if formMuse && (doc.editable ?? true)}
    {#if museError}<div transition:slide={{ duration: 150 }} class="mb-2 shrink-0 px-4 text-xs text-danger">{museError}</div>{/if}
    {#if museOpen}
      <!-- THE RECORD'S ONE MUSE — a chat composer, pinned above the menu like
           every other composer in the app: the submit tile INSIDE the prompt
           well, right-aligned, vertically centered — the wrapper hugs the
           textarea itself (not the px-4 gutter, which is how the tile ended
           up floating outside the input), and the anchor stretches the input's
           full height so centering holds as the well grows. -->
      <div transition:slide={{ duration: 150 }} class="mb-3 mt-2 shrink-0 px-4">
        <div class="relative">
          <Textarea
            autoGrow
            rows={1}
            bind:value={instruction}
            onkeydown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                if (!generating && instruction.trim()) void generate()
              }
            }}
            placeholder={proposal !== null
              ? 'Refine the proposal, e.g. “shorter, and add a step for weekends”'
              : `Describe the ${formMuse.label} you want — it fills every field; nothing is written until you save`}
            class="max-h-44 pr-12 text-sm"
            autofocus
          />
          <div class="absolute inset-y-0 right-1.5 flex items-center">
            {#if generating}
              <StopButton onClick={() => abortCtl?.abort()} />
            {:else}
              <SendButton enabled={!!instruction.trim()} onClick={() => void generate()} />
            {/if}
          </div>
        </div>
      </div>
    {/if}
  {/if}

  {#if failure}
    <div transition:slide={{ duration: 150 }} class="shrink-0 text-xs text-danger">
      {(failure as Error).message}
    </div>
  {/if}

  <!-- PINNED: the record's controls sit at the bottom of the surface and never
       scroll — the body above is the only thing that moves. FIXED HEIGHT, the
       sidebar's pinned footer row exactly: py-3 (24px) around its 28px `+`
       control plus the 1px border-t = 53px RENDERED, border-box. The two menus
       bookending the view must agree to the pixel or the frame reads broken. -->
  <div class="flex h-[53px] shrink-0 items-center gap-2 border-t border-line px-4">
    {#if formMuse && (doc.editable ?? true)}
      <Button
        variant={museOpen ? 'outline' : 'ghost'}
        size="sm"
        onclick={() => (museOpen = !museOpen)}
        title="Draft with AI. Uses your preferred model (Settings)"
      >
        <Sparkles size={14} class="mr-1.5" /> Muse
      </Button>
    {/if}
    <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
      {(doc.editable ?? true) && dirty ? 'Unsaved changes · ⌘S to save' : ''}
    </span>
    {#if onCancel}
      <Button variant="ghost" size="sm" onclick={cancel}>
        Cancel
      </Button>
    {/if}
    {#if onClose}
      <Button variant="ghost" size="sm" onclick={onClose}>
        Close
      </Button>
    {/if}
    {#if doc.editable ?? true}
      <Button size="sm" onclick={() => void saveAll()} disabled={busy || !dirty}>
        {busy ? 'Saving' : 'Save'}
      </Button>
    {/if}
  </div>
</div>
