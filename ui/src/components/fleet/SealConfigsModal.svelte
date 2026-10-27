<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { listStagger, slide } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  interface AgentSeal {
    agentId: string
    slug: string
    displayName: string
    status: 'clean' | 'planned' | 'sealed' | 'skipped'
    reason?: string
    versions: number[]
    sealed: string[]
    changed: string[]
    historyOnly: string[]
    dropped: string[]
    undeliverable: string[]
    roll: boolean
    rolled: boolean
  }
  interface SealReport {
    dryRun: boolean
    agents: AgentSeal[]
    warnings: string[]
  }

  // Configs stored before federation sealed inline credentials still hold
  // them as written, in every version. This asks the server what sealing them
  // would do (a dry run: nothing is written), then does it on the admin's say.
  // The server's order keeps a running agent working: secrets first, a roll
  // so the container holds them, and only then the configs.
  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  let report = $state<SealReport | null>(null)
  let busy = $state(false)
  let err = $state<string | null>(null)
  let dropUndeliverable = $state(false)

  const run = async (dryRun: boolean) => {
    busy = true
    err = null
    try {
      const j = await postJson<{ result?: SealReport }>('/api/fleet/seal-configs', { dryRun, dropUndeliverable })
      if (!j.result) {
        err = 'the server sent no report'
        return
      }
      report = j.result
      if (!dryRun) {
        await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
        await qc.invalidateQueries({ queryKey: ['agent-versions'] })
        await qc.invalidateQueries({ queryKey: ['agent-secrets'] })
        await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      }
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }

  // The dry run on open, and again when the undeliverable switch flips.
  $effect(() => {
    void dropUndeliverable
    void run(true)
  })

  const shown = $derived(report?.agents.filter((a) => a.status !== 'clean') ?? [])
  const clean = $derived((report?.agents.length ?? 0) - shown.length)
  const planned = $derived(shown.filter((a) => a.status === 'planned'))
  const rolls = $derived(planned.filter((a) => a.roll).length)
  const anyUndeliverable = $derived(shown.some((a) => a.undeliverable.length > 0))
  const applied = $derived(report !== null && !report.dryRun)

  const STATUS: Record<AgentSeal['status'], string> = {
    clean: 'clean',
    planned: 'to seal',
    sealed: 'sealed',
    skipped: 'skipped',
  }
</script>

<Modal open onClose={onClose} title="Seal credentials in stored configs" width="max-w-lg">
  <AutoHeight>
    <div class="space-y-4">
      <p class="text-sm leading-relaxed text-muted">
        Agents federated before credentials were sealed on the way in still store each inline credential as written, in every version of
        their config. Sealing makes each one a <code class="text-fg">CONFIG_…</code> secret of that agent and leaves a placeholder in every
        version. The value the agent runs on does not change. A running agent that gains secrets restarts once, before its config changes.
      </p>

      {#if report === null && busy}
        <p class="flex items-center gap-2 text-sm text-muted"><WaitingMark site="fleet/seal-configs" size={12} /> Reading stored configs</p>
      {/if}

      {#if report}
        <div class="text-sm text-fg">
          {#if applied}
            {shown.filter((a) => a.status === 'sealed').length} sealed · {shown.filter((a) => a.status === 'skipped').length} skipped · {clean} already clean
          {:else if shown.length === 0}
            Every stored config is clean: nothing to seal.
          {:else}
            {planned.length} to seal{rolls ? ` (${rolls} restart${rolls === 1 ? '' : 's'})` : ''} · {shown.length - planned.length} skipped · {clean} already clean
          {/if}
        </div>
        {#if shown.length > 0}
          <ul use:listStagger class="max-h-72 divide-y divide-line overflow-y-auto rounded-lg border border-line">
            {#each shown as a (a.agentId)}
              <li class="space-y-1 px-3.5 py-2 text-sm">
                <div class="flex items-center gap-2">
                  <span class="font-mono text-xs text-fg">{a.slug}</span>
                  {#if a.roll}
                    <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{a.rolled ? 'restarted' : 'restarts'}</span>
                  {/if}
                  <span
                    class={cn(
                      'ml-auto font-mono text-[10px] uppercase tracking-[0.05em]',
                      a.status === 'skipped' ? 'text-warning' : a.status === 'sealed' ? 'text-accent' : 'text-muted',
                    )}
                  >
                    {STATUS[a.status]}
                  </span>
                </div>
                {#if a.reason}<p class="text-xs leading-relaxed text-warning">{a.reason}</p>{/if}
                {#if a.sealed.length}
                  <p class="text-xs leading-relaxed text-muted">
                    Secrets: <span class="font-mono text-fg">{a.sealed.join(', ')}</span>
                  </p>
                {/if}
                {#if a.versions.length}
                  <p class="text-xs text-muted">Versions rewritten: {a.versions.map((v) => `v${v}`).join(', ')}</p>
                {/if}
                {#if a.changed.length}
                  <p class="text-xs leading-relaxed text-muted">
                    Older versions held a different value at <span class="font-mono text-fg">{a.changed.join('; ')}</span>: they now name the
                    current one.
                  </p>
                {/if}
                {#if a.historyOnly.length}
                  <p class="text-xs leading-relaxed text-muted">
                    Only older versions held <span class="font-mono text-fg">{a.historyOnly.join('; ')}</span>: the value is not kept, and
                    reverting to them leaves that setting out.
                  </p>
                {/if}
                {#if a.dropped.length}
                  <p class="text-xs leading-relaxed text-muted">
                    Removed, Talaria supplies these itself: <span class="font-mono text-fg">{a.dropped.join('; ')}</span>
                  </p>
                {/if}
                {#if a.undeliverable.length && a.status !== 'skipped'}
                  <p class="text-xs leading-relaxed text-muted">
                    Removed, too long or multi-line to seal: <span class="font-mono text-fg">{a.undeliverable.join('; ')}</span>
                  </p>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#each report.warnings as w (w)}
          <p class="text-xs text-warning">{w}</p>
        {/each}
        {#if anyUndeliverable && !applied}
          <Checkbox
            checked={dropUndeliverable}
            disabled={busy}
            onChange={(on) => (dropUndeliverable = on)}
            label="Take out credentials that cannot be sealed (multi-line or over 8 KB): that setting becomes unconfigured"
          />
        {/if}
      {/if}
      {#if err}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{err}</p>{/if}
    </div>
  </AutoHeight>

  {#snippet footer()}
    <div class="flex justify-end gap-2">
      {#if applied}
        <Button size="sm" onclick={onClose}>Done</Button>
      {:else}
        <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
        <Button size="sm" onclick={() => void run(false)} disabled={busy || planned.length === 0}>
          {#if busy && report}<WaitingMark site="fleet/seal-configs" size={12} />{/if}
          {busy && report ? 'Sealing' : `Seal ${planned.length} agent${planned.length === 1 ? '' : 's'}`}
        </Button>
      {/if}
    </div>
  {/snippet}
</Modal>
