<script lang="ts" module>
  // The table's captions (CronTable) and each row's cells share these widths.
  // The table is the container: a narrow one drops the agent, then the
  // frequency, which the row then says under its name.
  export const CRON_AGENT = 'hidden w-28 shrink-0 @4xl:block'
  export const CRON_FREQUENCY = 'hidden w-48 shrink-0 @3xl:block'
  export const CRON_NEXT = 'w-28 shrink-0 text-right'
  export const CRON_ON = 'w-9 shrink-0'
</script>

<script lang="ts">
  import { Pencil, Trash2, Zap } from '@lucide/svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { MAX_CRON_PROMPT, describeSchedule, editPatch, fmtNext, isRawSchedule, jobDot, parseSchedule, schedToString, type CronJob, type CronPatch, type Sched } from './agent-crons'
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
    /** Present ⇒ the row is editable; called on save with only the fields
     *  that changed (editPatch). */
    onEdit?: (patch: Partial<CronPatch>) => Promise<boolean>
    busy?: boolean
    agentLabel?: string
  } = $props()

  let expanded = $state(false)
  let editing = $state(false)
  // Draft state: seeded by startEdit() right before the edit form opens, so
  // these initializers are placeholders. The template only shows them while
  // editing is on — startEdit is the real seed.
  // svelte-ignore state_referenced_locally -- reason: edit drafts, re-seeded by startEdit() before they are ever shown
  let name = $state(job.name)
  // svelte-ignore state_referenced_locally -- reason: edit drafts, re-seeded by startEdit() before they are ever shown
  let sched = $state<Sched>(parseSchedule(job.schedule))
  // svelte-ignore state_referenced_locally -- reason: edit drafts, re-seeded by startEdit() before they are ever shown
  let prompt = $state(job.prompt)
  const paused = $derived(!job.enabled || job.state === 'paused')
  const failed = $derived(!paused && !!job.lastStatus && /error|fail/i.test(job.lastStatus))
  // A cadence the builder cannot name stays a cron string, which is an identifier.
  const rawCron = $derived(isRawSchedule(job.schedule))

  const startEdit = () => {
    name = job.name
    sched = parseSchedule(job.schedule)
    prompt = job.prompt
    editing = true
    expanded = true
  }
  const saveEdit = async () => {
    if (!onEdit) return
    const patch = editPatch(job, { name: name.trim(), schedule: schedToString(sched), prompt: prompt.trim() })
    // Nothing changed is nothing to send.
    if (Object.keys(patch).length === 0) {
      editing = false
      return
    }
    const ok = await onEdit(patch)
    if (ok) editing = false
  }

  const menu = (): ContextMenuEntry[] => [
    ...(onEdit ? [{ label: 'Edit', icon: [Pencil, { size: 14 }], disabled: busy, onSelect: startEdit } as ContextMenuEntry] : []),
    // A paused job cannot be run now: Hermes's run would resume it for good,
    // so the api refuses it. Resume first.
    { label: paused ? 'Run now (resume it first)' : 'Run now', icon: [Zap, { size: 14 }], disabled: busy || paused, onSelect: () => onAction?.('run') },
    'sep',
    { label: 'Delete', icon: [Trash2, { size: 14 }], danger: true, disabled: busy, onSelect: () => onAction?.('remove') },
  ]

  const cell = 'font-sans text-[13px] leading-5 text-muted'
  const cadence = $derived(describeSchedule(job.schedule))
</script>

{#snippet frequency(className: string)}
  <span class={cn(className, 'truncate text-muted', rawCron ? 'font-mono text-xs leading-5' : 'font-sans text-[13px] leading-5')} title={job.schedule}>
    {cadence}
  </span>
{/snippet}

<!-- A div, not an li: the schedules lists render these straight into a card
     that draws the dividers. -->
<div>
  <SettingsRow label={job.name} wrap onOpen={() => (expanded = !expanded)} {expanded}>
    {#snippet lead()}<StatusDot status={jobDot(job)} />{/snippet}
    {#snippet hint()}
      <!-- The failure is beside the name it is about, so the next run keeps its own column. -->
      {@render frequency('block @3xl:hidden')}
      {#if failed}<span class="block text-danger" title={job.lastError ?? undefined}>Last run failed</span>{/if}
    {/snippet}
    {#if agentLabel}<span class={cn(CRON_AGENT, cell, 'truncate')} title={agentLabel}>{agentLabel}</span>{/if}
    {@render frequency(CRON_FREQUENCY)}
    <span class={cn(CRON_NEXT, cell, 'tabular-nums')}>{paused ? 'Paused' : job.nextRunAt ? fmtNext(job.nextRunAt) : ''}</span>
    {#if onAction}
      <Toggle checked={!paused} disabled={busy} onChange={() => onAction(paused ? 'resume' : 'pause')} title={paused ? `Resume ${job.name}` : `Pause ${job.name}`} />
      <SettingsRowMenu label={`Actions for ${job.name}`} items={menu} />
    {/if}
  </SettingsRow>
  {#if expanded && !editing}
    <div class="space-y-2 pb-4 pl-[38px] pr-5" transition:slide={{ duration: 150 }}>
      <p class="max-w-[72ch] whitespace-pre-wrap font-sans text-[13px] leading-5 text-muted">{job.prompt}</p>
      <p class="font-sans text-xs leading-4 text-muted">
        {job.lastRunAt ? `Last ran ${relativeTime(job.lastRunAt)}${job.lastStatus ? ` · ${job.lastStatus}` : ''}` : 'Never ran'}
        {#if job.lastError}<span class="text-danger"> · {job.lastError}</span>{/if}
      </p>
    </div>
  {/if}
  {#if editing}
    <div class="space-y-5 pb-5 pl-[38px] pr-5 pt-1" transition:slide={{ duration: 150 }}>
      <SettingsField label="Name">
        <Input size="sm" bind:value={name} maxlength={80} class="w-[280px]" />
      </SettingsField>
      <SettingsField label="When">
        <ScheduleBuilder value={sched} onChange={(s) => (sched = s)} />
      </SettingsField>
      <SettingsField label="What it does">
        <Textarea autoGrow rows={3} bind:value={prompt} class="max-h-64" maxlength={MAX_CRON_PROMPT} />
      </SettingsField>
      <div class="flex items-center gap-2">
        <Button size="sm" disabled={busy || !name.trim() || !prompt.trim() || !schedToString(sched)} onclick={() => void saveEdit()}>
          Save changes
        </Button>
        <Button size="sm" variant="ghost" onclick={() => (editing = false)}>Cancel</Button>
      </div>
    </div>
  {/if}
</div>
