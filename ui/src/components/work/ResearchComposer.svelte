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
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Gauge, Telescope } from '@lucide/svelte'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import SendButton from '@/components/chat/SendButton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useHasPerm } from '@/lib/session'
  import { MODE_META, startResearch, type ResearchMode, type ResearchRun } from '@/lib/research'

  let {
    agentModel,
    onStarted,
    compact = false,
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
  } = $props()

  const qc = useQueryClient()
  const mayRun = useHasPerm('research.run')
  let question = $state('')
  let mode = $state<ResearchMode>('brief')
  let starting = $state(false)
  let error = $state<string | null>(null)

  const start = async () => {
    if (!question.trim() || !agentModel) return
    starting = true
    error = null
    try {
      const run = await startResearch(question.trim(), mode, agentModel)
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
  <div class="mt-2 flex h-10 min-w-0 items-center gap-1.5">
    <ComposerPicker
      icon={Gauge}
      value={mode}
      onChange={(m) => (mode = m as ResearchMode)}
      title="Research depth"
      menuLabel="Depth"
      options={(Object.keys(MODE_META) as ResearchMode[]).map((m) => ({
        value: m,
        label: MODE_META[m].label,
        sub: `${MODE_META[m].tagline} · ${MODE_META[m].eta}`,
      }))}
    />
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
