<script lang="ts">
  import { Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { slide } from '@/lib/motion'
  import { streamMuse } from '@/lib/muse.svelte'
  import { summarize } from '@/lib/agent-refine'

  /** Muse as the knowledge worker — ALWAYS present under the doc (read and
   *  edit): describe a change and it drafts from the current document. With a
   *  SELECTION in scope it works surgically — the proposal is a replacement for
   *  just that passage, applied in place on accept. Refinements keep short chat
   *  memory.
   *
   *  TALA-4: the accept used to be a vanishing act — the proposal card left
   *  the screen and the text underneath had changed with no word about what
   *  changed. Accept now folds with a change summary (±lines), and the
   *  Drafting state on the Refine button carries a pulse. */

  let {
    context,
    currentText,
    selection,
    surgical,
    onClearSelection,
    onAccept,
    onAcceptSelection,
  }: {
    context: string
    currentText: () => string
    selection?: string | null
    /** True when the selection can be replaced in place. */
    surgical?: boolean
    onClearSelection?: () => void
    onAccept: (markdown: string) => Promise<void>
    onAcceptSelection?: (replacement: string) => Promise<void>
  } = $props()

  let instruction = $state('')
  let proposal = $state<string | null>(null)
  let proposalScope = $state<'doc' | 'selection'>('doc')
  let generating = $state(false)
  let error = $state<string | null>(null)
  let chat = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  let abortCtl: AbortController | null = null
  $effect(() => () => abortCtl?.abort())

  // The accept's change summary, held until the next generate overwrites it.
  // `scope` rides alongside the RefineSummary (passage vs whole doc).
  let lastChange = $state<(ReturnType<typeof summarize> & { scope: 'doc' | 'selection' }) | null>(null)

  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || generating) return
    generating = true
    error = null
    lastChange = null
    const prevProposal = proposal
    const prevScope = proposalScope
    proposal = ''
    const scope = selection && surgical ? 'selection' : 'doc'
    proposalScope = scope
    const ac = new AbortController()
    abortCtl = ac
    try {
      const current =
        prevProposal !== null && prevScope === scope
          ? prevProposal
          : scope === 'selection'
            ? selection!
            : currentText()
      const ctx =
        scope === 'selection'
          ? `${context} You are editing ONLY this passage of the document — reply with the replacement passage alone, no commentary.`
          : selection
            ? `${context} Focus especially on this passage: «${selection.slice(0, 400)}»`
            : context
      const full = await streamMuse(
        { kind: 'document', context: ctx, current, instruction: instr, chat },
        (piece) => (proposal = (proposal ?? '') + piece),
        ac.signal,
      )
      chat = [...chat.slice(-10), { role: 'user', content: instr }, { role: 'assistant', content: full }]
      instruction = ''
    } catch (e) {
      if (!ac.signal.aborted) {
        error = (e as Error).message
        proposal = null
      }
    } finally {
      generating = false
    }
  }

  const accept = () => {
    if (proposal === null) return
    const md = proposal.trim()
    const scope = proposalScope
    const before = scope === 'selection' ? (selection ?? '') : currentText()
    lastChange = { ...summarize(before, md), scope }
    proposal = null
    if (scope === 'selection' && onAcceptSelection) void onAcceptSelection(md)
    else void onAccept(md)
  }
</script>

<div class="space-y-2 border-t border-line-subtle px-4 py-2.5">
  {#if selection}
    <div class="flex items-start gap-1.5 border-l-2 border-accent/50 pl-2 font-sans text-[11px] italic text-muted">
      <span class="min-w-0 flex-1 line-clamp-2">
        {surgical ? 'editing selection: ' : 'focused on: '}“{selection}”
      </span>
      <CloseButton
        onClick={() => onClearSelection?.()}
        size={11}
        label="Back to whole-document edits"
        class="shrink-0 p-0 hover:bg-transparent"
      />
    </div>
  {/if}
  <!-- §10 proposed-action card: hairline panel, gold left bar, mono label. -->
  {#if proposal !== null}
    <div class="rounded-lg border border-line border-l-2 border-l-accent bg-panel">
      <div class="px-3 pt-2 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Proposed draft</div>
      <div class="max-h-56 overflow-y-auto p-3 pt-1">
        <Markdown class="tiptap text-sm" children={proposal} />
      </div>
    </div>
  {/if}
  {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
  {#if lastChange}
    <!-- THE ACCEPT RECEIPT: what the accepted draft changed, in place of the
         silent swap this bar used to do. -->
    <div
      transition:slide={{ duration: 150 }}
      class="flex items-center gap-2 rounded-md border border-line-subtle bg-surface px-2.5 py-1.5 text-[11px] text-muted"
      data-muse-accepted="visible"
    >
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        {lastChange.scope === 'selection' ? 'Passage replaced' : 'Draft accepted'}
      </span>
      <span>
        {lastChange.adds} added · {lastChange.dels} removed
        line{lastChange.adds === 1 && lastChange.dels === 0 ? '' : 's'}
      </span>
    </div>
  {/if}
  <div class="flex items-end gap-2">
    <Sparkles size={14} class="mb-2.5 shrink-0 text-accent" />
    <Textarea
      autoGrow
      rows={1}
      bind:value={instruction}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          void generate()
        }
      }}
      placeholder={proposal !== null
        ? 'Refine the proposal, e.g. “tighter, and add a checklist”'
        : selection
          ? surgical
            ? 'How should this passage change?'
            : 'What about this passage? (it will guide a whole-doc draft)'
          : 'Edit with Muse: describe the change, or select text for inline edits'}
      class="max-h-32 text-sm"
    />
    <Button
      size="sm"
      class="shrink-0"
      onclick={() => void generate()}
      disabled={generating || !instruction.trim()}
      data-muse-busy={generating ? 'true' : undefined}
    >
      <span class:gd-pulse={generating}>
        {generating ? 'Drafting' : proposal !== null ? 'Refine' : 'Draft'}
      </span>
    </Button>
    {#if proposal !== null && !generating}
      <Button size="sm" variant="outline" class="shrink-0" onclick={accept}>
        {proposalScope === 'selection' ? 'Replace passage' : 'Accept'}
      </Button>
      <Button size="sm" variant="ghost" class="shrink-0" onclick={() => (proposal = null)}>
        Discard
      </Button>
    {/if}
  </div>
</div>
