<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { DAYS, describeSchedule, schedToString, type Sched, type SchedMode } from './agent-crons'

  // The scheduling UI: pick a shape, fill the blanks, see it in English.
  let { value, onChange }: { value: Sched; onChange: (s: Sched) => void } = $props()

  const set = (patch: Partial<Sched>) => onChange({ ...value, ...patch })
</script>

<div class="space-y-2">
  <div class="flex flex-wrap items-center gap-2">
    <Select size="sm" value={value.mode} onchange={(e) => set({ mode: e.currentTarget.value as SchedMode })} class="w-36">
      <option value="interval">Repeating</option>
      <option value="daily">Every day</option>
      <option value="weekdays">Weekdays</option>
      <option value="weekly">Weekly</option>
      <option value="monthly">Monthly</option>
      <option value="custom">Custom (cron)</option>
    </Select>
    {#if value.mode === 'interval'}
      <span class="text-xs text-muted">every</span>
      <Input size="sm" type="number" min={1} max={999} value={value.every} oninput={(e) => set({ every: Math.max(1, Number(e.currentTarget.value) || 1) })} class="w-20" />
      <Select size="sm" value={value.unit} onchange={(e) => set({ unit: e.currentTarget.value as 'm' | 'h' })} class="w-28">
        <option value="m">minutes</option>
        <option value="h">hours</option>
      </Select>
    {/if}
    {#if value.mode === 'weekly'}
      <Select size="sm" value={String(value.dow)} onchange={(e) => set({ dow: Number(e.currentTarget.value) })} class="w-32">
        {#each DAYS as d, i (d)}
          <option value={i}>{d}</option>
        {/each}
      </Select>
    {/if}
    {#if value.mode === 'monthly'}
      <span class="text-xs text-muted">on day</span>
      <Input size="sm" type="number" min={1} max={31} value={value.dom} oninput={(e) => set({ dom: Math.min(31, Math.max(1, Number(e.currentTarget.value) || 1)) })} class="w-16" />
    {/if}
    {#if ['daily', 'weekdays', 'weekly', 'monthly'].includes(value.mode)}
      <span class="text-xs text-muted">at</span>
      <Input size="sm" type="time" value={value.time} oninput={(e) => set({ time: e.currentTarget.value || '09:00' })} class="w-28" />
    {/if}
    {#if value.mode === 'custom'}
      <Input size="sm" value={value.custom} oninput={(e) => set({ custom: e.currentTarget.value })} placeholder="0 9 * * 1-5" class="w-44 font-mono" />
    {/if}
    <InfoTip text="Times are the agent's clock (UTC). Underneath this compiles to standard cron syntax — Custom accepts any 5-field expression or an interval like 'every 2h'." />
  </div>
  <div class="text-xs text-muted">
    → <span class="font-sans text-fg">{describeSchedule(schedToString(value)) || '…'}</span>
    {#if value.mode !== 'custom'}<span class="ml-2 font-mono text-[10px] tracking-[0.05em] text-ink-dim">{schedToString(value)}</span>{/if}
  </div>
</div>
