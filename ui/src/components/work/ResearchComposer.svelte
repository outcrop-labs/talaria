<script lang="ts">
  // THE RESEARCH ASK, as a block any surface can host. It was the zero state
  // of the research page and lived nowhere else; Work needs the same question
  // box without owning a page, so the ask moved out here and the host keeps
  // what surrounds it — the centering, the heading, and what to do with the
  // run that comes back.
  //
  // §7 composer anatomy, unchanged from the page it came from: panel body,
  // STRONG hairline, radius 8, p-2, matte float shadow — the prompt in a
  // ground inset on top, every control on the row UNDERNEATH it, submit
  // right-aligned. It fills the width it is given; a host that wants the
  // centred reading wraps it in the converse-width column it always had.
  //
  // THE DEPTH PICKER AND THE THREE DIALS ARE ONE DECISION, read two ways. A
  // mode is a preset over the dials (recon 1/1/1, brief 1/2/2, expedition
  // 3/3/3), so picking one RESETS all three — that is what choosing a preset
  // means — and moving a dial afterwards keeps the preset's name on the chip
  // while the body carries the numbers. The name is still the honest label of
  // where the ask started, and the run itself records the dials it ran at.
  import { useQueryClient } from '@tanstack/svelte-query'
  import type { LucideIcon as IconType } from '@lucide/svelte'
  import { Gauge, Library, Pickaxe, Radar, Telescope } from '@lucide/svelte'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import SendButton from '@/components/chat/SendButton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useHasPerm } from '@/lib/session'
  import {
    DIAL_AXES,
    MODE_META,
    dialsForMode,
    startResearch,
    type ResearchDials,
    type ResearchMode,
    type ResearchRun,
  } from '@/lib/research'

  let {
    agentModel,
    onStarted,
    compact = false,
    conversationId = null,
  }: {
    /** WHOSE EXPERTISE DRIVES THE RUN. Null is a real state, not a missing
     *  prop — "all agents" on a rail picker, or a workspace with none yet —
     *  and research needs a concrete runner, so send waits for a pick rather
     *  than choosing one on the person's behalf. */
    agentModel: string | null
    /** The run the server created. What it MEANS is the host's call: the
     *  research page selects it, a work surface may only need its list to
     *  show it. The composer has already cleared itself by then. */
    onStarted: (run: ResearchRun) => void
    /** Embedded beside other things rather than owning the middle of a stage:
     *  the prompt well opens at three rows instead of six. Only the well
     *  changes — the control row stays the same row, so the two readings of
     *  this composer remain the same control. */
    compact?: boolean
    /** THE THREAD THE RUN REPORTS INTO, when the ask was made from inside one:
     *  the scope question, the ack and the report-ready turn land in the
     *  session the person asked from instead of in a thread of the run's own.
     *  Null — the ask from a stage with nothing open — is the run making its
     *  own, which is what it has always done. */
    conversationId?: string | null
  } = $props()

  const qc = useQueryClient()
  const mayRun = useHasPerm('research.run')
  let question = $state('')
  let mode = $state<ResearchMode>('brief')
  // Seeded from the mode above; pickMode() reseeds it on every later change.
  // svelte-ignore state_referenced_locally -- reason: the initial preset, re-read only by pickMode
  let dials = $state<ResearchDials>(dialsForMode(mode))
  let starting = $state(false)
  let error = $state<string | null>(null)

  // The action register for the dials (UI-CONVENTIONS, icon language): a dig,
  // a sweep, a shelf. The axes themselves live in lib/research — a plain
  // module cannot hold components, and the pairing is this surface's.
  const DIAL_ICON: Record<keyof ResearchDials, IconType> = {
    effort: Pickaxe,
    breadth: Radar,
    sources: Library,
  }

  /** Picking a preset is picking its numbers: the whole point of a mode is
   *  that it sets all three, so a stale dial surviving the change would make
   *  "Expedition" mean whatever the last ask happened to leave behind. */
  const pickMode = (m: ResearchMode) => {
    mode = m
    dials = dialsForMode(m)
  }

  const start = async () => {
    if (!question.trim() || !agentModel) return
    starting = true
    error = null
    try {
      // ALL THREE DIALS, every time. The route takes a partial and fills the
      // rest from the preset, so either shape is correct — sending all three
      // is the one that cannot be read two ways once a dial has moved.
      const run = await startResearch(question.trim(), mode, agentModel, { ...dials }, conversationId ?? undefined)
      question = ''
      onStarted(run)
      // The runs list is stale the moment a run exists, and every host shows
      // that list somewhere — so the composer invalidates it itself rather
      // than leaving each caller to remember.
      void qc.invalidateQueries({ queryKey: ['research-runs'] })
    } catch (e) {
      error = (e as Error).message
    } finally {
      starting = false
    }
  }
</script>

<div class="rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]">
  <div class="rounded-md border border-line bg-surface px-2 py-1">
    <Textarea
      autoGrow
      rows={compact ? 3 : 6}
      bind:value={question}
      disabled={!mayRun.current}
      placeholder={mayRun.current ? 'What should we find out?' : 'You don’t have permission to run research'}
      class={cn(
        'border-0 bg-transparent focus:border-0',
        compact ? 'max-h-[16rem] min-h-[5rem]' : 'max-h-[26rem] min-h-[10rem]',
      )}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          void start()
        }
      }}
    />
  </div>
  <div class="mt-2 flex min-h-10 min-w-0 flex-wrap items-center gap-1.5">
    <ComposerPicker
      icon={Gauge}
      value={mode}
      onChange={(m) => pickMode(m as ResearchMode)}
      title="Research depth"
      menuLabel="Depth"
      options={(Object.keys(MODE_META) as ResearchMode[]).map((m) => ({
        value: m,
        label: MODE_META[m].label,
        sub: `${MODE_META[m].tagline} · ${MODE_META[m].eta}`,
      }))}
    />
    <!-- The preset, axis by axis. Each chip reads its axis name with the
         ladder beside it, so three rungs of three are one glance; the rows
         say what the rung actually buys. Autosave posture (UI-CONVENTIONS):
         a dial applies on change and travels with the next ask — nothing
         here spends anything until Start. -->
    {#each DIAL_AXES as axis (axis.key)}
      <ComposerPicker
        icon={DIAL_ICON[axis.key]}
        value={String(dials[axis.key])}
        label={axis.label}
        onChange={(v) => (dials = { ...dials, [axis.key]: Number(v) })}
        title={axis.note}
        menuLabel={axis.label}
        meter={{ total: axis.steps.length, lit: dials[axis.key] }}
        searchable={false}
        menuClass="min-w-48"
        options={axis.steps.map((step, i) => ({ value: String(i + 1), label: step }))}
      />
    {/each}
    {#if starting}
      <span class="grid h-9 w-9 shrink-0 place-items-center"><WaitingMark site="research/start" size={12} class="text-accent" /></span>
    {/if}
    <span class="flex-1"></span>
    <SendButton
      title="Start (⏎)"
      icon={Telescope}
      enabled={mayRun.current && !starting && !!question.trim() && !!agentModel}
      onClick={() => void start()}
    />
  </div>
  {#if error}<div transition:slide={{ duration: 150 }} class="px-2 pb-1 pt-1 text-xs text-danger">{error}</div>{/if}
</div>
