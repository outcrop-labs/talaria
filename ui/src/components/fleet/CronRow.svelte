<script lang="ts">
  import { Pause, Pencil, Play, Trash2, Zap } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { describeSchedule, fmtNext, jobDot, parseSchedule, schedToString, type CronJob, type Sched } from './agent-crons'
  import ScheduleBuilder from './ScheduleBuilder.svelte'

  let {
    job,
    onAction,
    onEdit,
    busy,
    agentLabel,
  }: {
    job: CronJob
    onAction?: (action: 'pause' | 'resume' | 'run' | 'remove') => void
    /** Present ⇒ the row is editable; called with the patch on save. */
    onEdit?: (patch: { name: string; schedule: string; prompt: string }) => Promise<boolean>
    busy?: boolean
    agentLabel?: string
  } = $props()

  let expanded = $state(false)
  let editing = $state(false)
  let name = $state(job.name)
  let sched = $state<Sched>(parseSchedule(job.schedule))
  let prompt = $state(job.prompt)
  const paused = $derived(!job.enabled || job.state === 'paused')

  const startEdit = () => {
    name = job.name
    sched = parseSchedule(job.schedule)
    prompt = job.prompt
    editing = true
    expanded = true
  }
  const saveEdit = async () => {
    if (!onEdit) return
    const ok = await onEdit({ name: name.trim(), schedule: schedToString(sched), prompt: prompt.trim() })
    if (ok) editing = false
  }
</script>

<li class="px-3.5 py-3">
  <div class="flex items-center gap-2.5">
    <span class="h-[7px] w-[7px] shrink-0 rounded-full" style:background={jobDot(job)} title={paused ? 'paused' : job.state}></span>
    <button type="button" onclick={() => (expanded = !expanded)} class="min-w-0 flex-1 text-left">
      <span class="font-sans text-sm font-medium text-fg">{job.name}</span>
      {#if agentLabel}<span class="ml-2 text-xs text-muted">{agentLabel}</span>{/if}
      <span class="ml-2 font-sans text-xs text-accent" title={job.schedule}>
        {describeSchedule(job.schedule)}
      </span>
    </button>
    <span class="shrink-0 font-mono text-[11px] text-muted">
      {paused ? 'paused' : job.nextRunAt ? `next ${fmtNext(job.nextRunAt)}` : ''}
    </span>
    {#if onAction}
      <span class={cn('flex shrink-0 items-center', busy && 'pointer-events-none opacity-40')}>
        {#if onEdit}
          <button
            type="button"
            title="Edit"
            onclick={startEdit}
            class="grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
          >
            <Pencil size={13} />
          </button>
        {/if}
        <button
          type="button"
          title="Run on the next tick (≤60s)"
          onclick={() => onAction('run')}
          class="grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <Zap size={14} />
        </button>
        <button
          type="button"
          title={paused ? 'Resume' : 'Pause'}
          onclick={() => onAction(paused ? 'resume' : 'pause')}
          class="grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          {#if paused}<Play size={14} fill="currentColor" />{:else}<Pause size={14} fill="currentColor" />{/if}
        </button>
        <button
          type="button"
          title="Delete"
          onclick={() => onAction('remove')}
          class="grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-danger"
        >
          <Trash2 size={14} />
        </button>
      </span>
    {/if}
  </div>
  {#if expanded && !editing}
    <div class="mt-2.5 space-y-1.5 pl-4" transition:slide={{ duration: 150 }}>
      <div class="whitespace-pre-wrap rounded-md border border-line p-3 font-sans text-xs leading-5 text-muted">{job.prompt}</div>
      <div class="font-mono text-[11px] text-muted">
        {job.lastRunAt ? `last ran ${relativeTime(job.lastRunAt)}${job.lastStatus ? ` · ${job.lastStatus}` : ''}` : 'never ran'}
        {#if job.lastError}<span class="text-danger"> · {job.lastError}</span>{/if}
      </div>
    </div>
  {/if}
  {#if editing}
    <div class="mt-2.5 space-y-3 rounded-lg border border-line p-3.5 pl-4" transition:slide={{ duration: 150 }}>
      <div class="flex items-center gap-2">
        <Input size="sm" bind:value={name} maxlength={80} class="w-56" />
      </div>
      <ScheduleBuilder value={sched} onChange={(s) => (sched = s)} />
      <Textarea autoGrow rows={3} bind:value={prompt} class="max-h-64" maxlength={20_000} />
      <div class="flex items-center gap-2">
        <Button size="sm" disabled={busy || !name.trim() || !prompt.trim() || !schedToString(sched)} onclick={() => void saveEdit()}>
          Save changes
        </Button>
        <Button size="sm" variant="ghost" onclick={() => (editing = false)}>
          Cancel
        </Button>
      </div>
    </div>
  {/if}
</li>
