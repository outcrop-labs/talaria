<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { slide } from '@/lib/motion'
  import { draftCron } from '@/lib/muse.svelte'
  import { DEFAULT_SCHED, parseSchedule, schedToString, type Sched } from './agent-crons'
  import ScheduleBuilder from './ScheduleBuilder.svelte'

  let {
    onCreate,
    busy,
    disabled = false,
    children,
  }: {
    onCreate: (input: { name: string; schedule: string; prompt: string }) => Promise<boolean>
    busy: boolean
    /** Hold the create button (e.g. while the fleet agent list is still loading,
     *  when the target set would silently be empty). */
    disabled?: boolean
    /** Extra fields (e.g. the fleet agent picker) rendered above the buttons. */
    children?: Snippet
  } = $props()

  let name = $state('')
  let sched = $state<Sched>(DEFAULT_SCHED)
  let prompt = $state('')
  let draftAsk = $state('')
  let drafting = $state(false)
  let draftErr = $state<string | null>(null)
  const ok = $derived(name.trim() && schedToString(sched) && prompt.trim())

  // Natural language → {name, schedule, prompt}, validated server-side. A
  // failed draft leaves the form exactly as it was and says why — the muse is
  // the shortcut here, never the only way in.
  const draft = async () => {
    const ask = draftAsk.trim()
    if (!ask) return
    drafting = true
    draftErr = null
    try {
      const j = await draftCron({ instruction: ask })
      name = j.name
      sched = parseSchedule(j.schedule)
      prompt = j.prompt
      draftAsk = ''
    } catch (e) {
      draftErr = (e as Error).message
    } finally {
      drafting = false
    }
  }
</script>

<div class="space-y-5 rounded-lg border border-line p-5">
  <div class="flex items-end gap-2.5">
    <Sparkles size={14} class="mb-3 shrink-0 text-accent" />
    <Textarea
      autoGrow
      rows={1}
      bind:value={draftAsk}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          if (!drafting && draftAsk.trim()) void draft()
        }
      }}
      placeholder="Describe it, e.g. “every weekday morning, summarize my inbox into a brief”"
      class="max-h-32 text-sm"
    />
    <Button variant="outline" class="shrink-0 whitespace-nowrap" onclick={() => void draft()} disabled={drafting || !draftAsk.trim()}>
      {drafting ? 'Drafting' : 'Draft'}
    </Button>
  </div>
  {#if draftErr}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{draftErr}</p>{/if}
  {#if drafting}<Generating site="fleet/cron-design" label="Designing the job: name, schedule, and the prompt it runs" lines={2} />{/if}

  <div class="grid gap-4 sm:grid-cols-2">
    <div>
      <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
      <Input bind:value={name} placeholder="weekly-recap" maxlength={80} />
    </div>
  </div>
  <div>
    <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">When</label>
    <ScheduleBuilder value={sched} onChange={(s) => (sched = s)} />
  </div>
  <div>
    <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">What it does</label>
    <Textarea
      autoGrow
      rows={3}
      bind:value={prompt}
      placeholder="What should it do each time? Written as a self-contained instruction."
      class="max-h-64"
      maxlength={20_000}
    />
  </div>
  {@render children?.()}
  <div class="flex items-center gap-3">
    <Button
      class="shrink-0 whitespace-nowrap"
      disabled={!ok || busy || disabled}
      onclick={() =>
        void onCreate({ name: name.trim(), schedule: schedToString(sched), prompt: prompt.trim() }).then((created) => {
          if (created) {
            name = ''
            sched = DEFAULT_SCHED
            prompt = ''
          }
        })}
    >
      {#if busy}<WaitingMark site="fleet/cron-save" size={12} />{/if}
      {busy ? 'Creating' : 'Create job'}
    </Button>
  </div>
</div>
