<script lang="ts">
  import { Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import { draftTicketPatch, streamMuse, type TicketMusePatch } from '@/lib/muse.svelte'
  import type { Task } from '@/lib/task-const'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'

  /** Muse on tickets — one bar, two modes. With a description selection (while
   *  editing): rewrite just that passage. Otherwise: natural-language FIELD
   *  edits ("urgent, due friday, label launch") proposed as a previewable patch. */
  let {
    t,
    editor,
    onPatch,
  }: {
    t: Task
    /** The live description editor handle (was a React ref; a reactive prop here). */
    editor: RichEditorHandle | null
    onPatch: (patch: TicketMusePatch) => Promise<void>
  } = $props()

  let instruction = $state('')
  let generating = $state(false)
  let error = $state<string | null>(null)
  let fieldPatch = $state<TicketMusePatch | null>(null)
  let passage = $state<string | null>(null)
  let abortCtl: AbortController | null = null
  // Abort any in-flight stream when the bar unmounts (React's [] effect).
  $effect(() => () => abortCtl?.abort())

  const FIELD_LABEL: Record<string, string> = {
    title: 'title',
    priority: 'priority',
    effort: 'effort',
    estimatedHours: 'estimate',
    dueDate: 'due',
    startDate: 'start',
    color: 'color',
    tags: 'labels',
    status: 'status',
  }

  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || generating) return
    generating = true
    error = null
    fieldPatch = null
    passage = null
    const ac = new AbortController()
    abortCtl = ac
    const sel = editor?.getSelectionText().trim()
    try {
      if (sel) {
        // Selection mode: rewrite only the selected passage of the description.
        let acc = ''
        acc = await streamMuse(
          {
            kind: 'document',
            context:
              'You are editing ONLY this selected passage of a project ticket description — reply with the replacement passage alone, no commentary.',
            current: sel,
            instruction: instr,
          },
          () => {},
          ac.signal,
        )
        passage = acc.trim()
      } else {
        // Field mode: structured JSON patch over the whole ticket.
        const current = JSON.stringify({
          title: t.title,
          description: t.description,
          priority: t.priority,
          effort: t.effort,
          estimatedHours: t.estimatedHours,
          dueDate: t.dueDate,
          startDate: t.startDate,
          color: t.color,
          tags: t.tags,
          status: t.status,
        })
        // Validated server-side against the FIELD ALLOWLIST — the patch that
        // comes back can only ever touch fields this bar is allowed to change,
        // and `error` is how the Muse says the instruction asked for something
        // else (an assignee, a board move) instead of inventing a plausible
        // edit. Nothing is written either way until Apply.
        const patch = await draftTicketPatch({ context: `now: ${new Date().toISOString()}`, current, instruction: instr }, ac.signal)
        if (patch.error) throw new Error(patch.error)
        fieldPatch = patch
      }
      instruction = ''
    } catch (e) {
      if (!ac.signal.aborted) error = (e as Error).message
    } finally {
      generating = false
    }
  }

  // THE PREVIEW SURVIVES UNTIL THE WRITE LANDS. `onPatch` is `updateTask`, which
  // THROWS on a non-2xx: clearing `fieldPatch` first and calling it from
  // `onclick={() => void applyFields()}` turned a 400 from PUT /api/tasks/:id
  // into an unhandled rejection with the chips already gone — the user watched
  // the preview vanish and nothing happen, and the patch was unrecoverable.
  // The harness contract now catches the likeliest causes before the model's
  // answer ever gets here (muse.ts's schema is the route's `Patch`, bound for
  // bound), but a board with a custom status set can still refuse a status this
  // schema allows, and "rare" is not "never" for the one step that writes.
  const applyFields = async () => {
    if (!fieldPatch) return
    const p = fieldPatch
    error = null
    try {
      await onPatch(p)
      fieldPatch = null
    } catch (e) {
      error = (e as Error).message
    }
  }
  const applyPassage = () => {
    if (passage === null) return
    editor?.replaceSelection(passage)
    passage = null
    const md = editor?.getMarkdown()
    // Same swallow as `applyFields` had, one field narrower: the replacement is
    // already in the editor, so a rejected write leaves the screen disagreeing
    // with the server until something refetches. Saying so is the whole fix.
    if (md !== undefined) void onPatch({ description: md }).catch((e: unknown) => (error = (e as Error).message))
  }
</script>

{#snippet chip(k: string, v: unknown)}
  <span class="rounded-full border border-accent/40 bg-accent/10 px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-fg">
    {FIELD_LABEL[k] ?? k} → {v === null ? 'clear' : Array.isArray(v) ? v.join(', ') : String(v).slice(0, 40)}
  </span>
{/snippet}

<div class="shrink-0 space-y-2 border-t border-line-subtle px-5 py-2.5">
  {#if fieldPatch}
    <div transition:slide={{ duration: 150 }} class="space-y-2 rounded-lg border border-accent/30 bg-card/40 p-3">
      <div class="flex flex-wrap items-center gap-1.5">
        {#each Object.entries(fieldPatch).filter(([k]) => k !== 'description') as [k, v] (k)}
          {@render chip(k, v)}
        {/each}
      </div>
      {#if fieldPatch.description !== undefined}
        <div class="max-h-40 overflow-y-auto rounded-lg border border-line bg-card px-3 py-2">
          <Markdown class="font-sans text-sm" children={fieldPatch.description ?? ''} />
        </div>
      {/if}
      <div class="flex justify-end gap-2">
        <Button size="sm" variant="outline" onclick={() => (fieldPatch = null)}>
          Discard
        </Button>
        <Button size="sm" onclick={() => void applyFields()}>
          Apply
        </Button>
      </div>
    </div>
  {/if}
  {#if passage !== null}
    <div transition:slide={{ duration: 150 }} class="space-y-2 rounded-lg border border-accent/30 bg-card/40 p-3">
      <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Replacement for the selection</div>
      <div class="max-h-40 overflow-y-auto">
        <Markdown class="font-sans text-sm" children={passage} />
      </div>
      <div class="flex justify-end gap-2">
        <Button size="sm" variant="outline" onclick={() => (passage = null)}>
          Discard
        </Button>
        <Button size="sm" onclick={applyPassage}>
          Replace selection
        </Button>
      </div>
    </div>
  {/if}
  {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-xs text-danger">{error}</div>{/if}
  <div class="flex items-center gap-2">
    <Sparkles size={14} class={cn('shrink-0 text-accent', generating && 'gd-pulse')} />
    <Input
      size="sm"
      bind:value={instruction}
      onkeydown={(e) => e.key === 'Enter' && void generate()}
      placeholder={generating ? 'Muse is thinking' : 'Muse: "urgent, due friday" — or select text while editing to rewrite it'}
      disabled={generating}
      class="flex-1"
    />
  </div>
</div>
