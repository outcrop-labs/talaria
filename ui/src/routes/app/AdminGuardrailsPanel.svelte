<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { tableCell, tableHead, tableRow } from '@/components/admin/table'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'

  interface GuardData {
    config: { mode: string; checks: Record<string, boolean>; minConfidence: number; policedHosts: string[]; coach: boolean }
    stats: { total: number; byCheck: Record<string, number> }
    findings: Array<{ id: string; caller: string; model: string; check: string; severity: string; confidence: number; message: string; snippet: string; createdAt: string }>
    rules: Array<{ id: string; label: string; severity: string; defaultOn: boolean }>
  }

  // Confab guardrail — a structural check on model output at the gateway. Observe
  // mode records findings out-of-band; annotate/strict act on them.
  const qc = useQueryClient()
  const autosave = useAutosave()
  const query = createQuery(() => ({
    queryKey: ['guardrails'],
    queryFn: (): Promise<GuardData> => getJson<GuardData>('/api/admin/guardrails'),
    refetchInterval: 30_000,
  }))
  const data = $derived(query.data)
  const cfg = $derived(data?.config)
  const save = async (row: string, patch: Partial<GuardData['config']>) => {
    if (!cfg) return
    const body = { ...cfg, ...patch }
    try {
      await putJson<{ config: GuardData['config'] }>('/api/admin/guardrails', body)
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['guardrails'] })
    autosave.flash(row)
  }
  const rules = $derived(data?.rules ?? [])
  const off = $derived(cfg?.mode === 'off')
  let showFindings = $state(false)

  // The header's captions and each row's cells share these widths.
  const CATCHES = 'w-[19rem] shrink-0 truncate'
  const NUM = 'w-[4.5rem] shrink-0 text-right'
  const SWITCH = 'flex w-14 shrink-0 justify-end'
  const cell = 'font-sans text-[13px] leading-5'

  const MODE_HINT: Record<string, string> = {
    off: 'No replies are checked.',
    observe: 'Findings are recorded and replies are left alone.',
    annotate: 'Findings are recorded and the reply is flagged.',
    strict: 'Findings are recorded and the flagged text is removed.',
  }
  // A rule's label carries its explanation in brackets: "Secret leak
  // (credential in output)".
  const ruleParts = (label: string): { name: string; catches: string } => {
    const m = /^(.*?)\s*\((.*)\)$/.exec(label)
    const catches = m?.[2] ?? ''
    return { name: m?.[1] ?? label, catches: catches.charAt(0).toUpperCase() + catches.slice(1) }
  }
  const checkName = (id: string) => {
    const rule = rules.find((r) => r.id === id)
    if (rule) return ruleParts(rule.label).name
    const words = id.replace(/_/g, ' ')
    return words.charAt(0).toUpperCase() + words.slice(1)
  }
</script>

<SettingsSection
  title="Guardrails"
  description="Checks every model reply for made-up tool results, invented links and leaked secrets."
  info="A check on every model reply at the gateway: claims of work no tool did, invented links, outage stories, leaked secrets and personal data. Flagged content never goes back into an agent's context."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the guardrail configuration" errorVariant="compact">
      {#snippet skeleton()}
        <!-- Everything here seeds from the query (mode, slider, coach toggle,
             rules) — hold the rows so no false defaults flash. -->
        <div class="px-5 py-4"><SkeletonRows rows={6} /></div>
      {/snippet}
      {#snippet children(d)}
        <!-- "0 findings" and mode=observe are both defaults this component
             invents when the read failed — over a failure that reads as a
             guard quietly running and finding nothing. QueryState's error
             branch holds it. -->
        <SettingsRow label="Mode" hint={MODE_HINT[cfg?.mode ?? 'observe']} as="label" saved={autosave.saved('mode')}>
          <Select size="sm" value={cfg?.mode ?? 'observe'} onchange={(e) => void save('mode', { mode: e.currentTarget.value })} class="w-[280px]">
            <option value="off">Off</option>
            <option value="observe">Observe</option>
            <option value="annotate">Annotate</option>
            <option value="strict">Strict</option>
          </Select>
        </SettingsRow>
        <SettingsRow label="Minimum confidence" hint="Findings the check is less sure of than this are dropped." as="label" saved={autosave.saved('confidence')}>
          <input
            type="range" min={0} max={1} step={0.05}
            value={cfg?.minConfidence ?? 0.5}
            disabled={off}
            onchange={(e) => void save('confidence', { minConfidence: Number(e.currentTarget.value) })}
            class="w-40 accent-accent disabled:opacity-50"
          />
          <span class="w-10 text-right font-sans text-[13px] leading-5 tabular-nums text-fg">{((cfg?.minConfidence ?? 0.5) * 100).toFixed(0)}%</span>
        </SettingsRow>
        <SettingsRow
          label="Coach agents"
          hint="Repeated findings become advice in the agent's instructions."
          info="Only counts and advice are added, never the flagged content. It applies the next time your agents are restarted."
          saved={autosave.saved('coach')}
        >
          <Toggle checked={cfg?.coach ?? false} disabled={off} onChange={(checked) => void save('coach', { coach: checked })} title="Coach agents from findings" />
        </SettingsRow>
        <SettingsHeaderRow label="Check" class="h-9">
          <span class={CATCHES}>Catches</span>
          <span class={NUM}>Findings</span>
          <span class={SWITCH}></span>
        </SettingsHeaderRow>
        {#each rules as c (c.id)}
          {@const r = ruleParts(c.label)}
          <SettingsRow saved={autosave.saved(c.id)} class="min-h-11 py-1.5">
            {#snippet label()}<span class="text-[13px]">{r.name}</span>{/snippet}
            <span class="{CATCHES} {cell} text-muted" title={r.catches}>{r.catches}</span>
            <span class={cn(NUM, cell, 'tabular-nums', d.stats.byCheck[c.id] ? 'text-fg' : 'text-muted')}>
              {(d.stats.byCheck[c.id] ?? 0).toLocaleString()}
            </span>
            <span class={SWITCH}>
              <Toggle
                checked={cfg?.checks[c.id] ?? c.defaultOn}
                disabled={off}
                onChange={(checked) => cfg && void save(c.id, { checks: { ...cfg.checks, [c.id]: checked } })}
                title={`Run the ${r.name} check`}
              />
            </span>
          </SettingsRow>
        {/each}
        {#if d.findings.length > 0}
          <SettingsRow
            label="Recent findings"
            hint="{d.stats.total.toLocaleString()} recorded in all."
            onclick={() => (showFindings = !showFindings)}
            expanded={showFindings}
          />
          {#if showFindings}
            <div transition:slide={{ duration: 150 }} class="max-h-[352px] overflow-y-auto">
              <table class="w-full table-fixed">
                <colgroup>
                  <col class="w-56" />
                  <col />
                  <col class="w-44" />
                  <col class="w-28" />
                </colgroup>
                <thead>
                  <tr>
                    <th class={tableHead}>Check</th>
                    <th class={tableHead}>Reply</th>
                    <th class={tableHead}>Model</th>
                    <th class={cn(tableHead, 'text-right')}>When</th>
                  </tr>
                </thead>
                <tbody>
                  {#each d.findings.slice(0, 20) as f (f.id)}
                    <tr class={tableRow}>
                      <td class={tableCell}>
                        <span class="flex items-center gap-2">
                          <StatusDot status={f.severity === 'high' ? 'danger' : 'warn'} title="{f.severity} severity" />
                          <span class="truncate">{checkName(f.check)}</span>
                        </span>
                      </td>
                      <td class={cn(tableCell, 'truncate text-muted')} title={f.snippet}>{f.snippet || f.message}</td>
                      <td class={cn(tableCell, 'truncate font-mono text-xs text-muted')}>{f.model}</td>
                      <td class={cn(tableCell, 'whitespace-nowrap text-right text-muted')}>{relativeTime(f.createdAt)}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
