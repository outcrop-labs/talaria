<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { ChipTone } from '@/components/ui/chip'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { listStagger, slide } from '@/lib/motion'

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
    /** Its container exists, stopped: applying removes it, so its next start creates one that holds the secrets. */
    removeStopped: boolean
    removed: boolean
  }
  interface SealReport {
    dryRun: boolean
    agents: AgentSeal[]
    warnings: string[]
  }

  // Configs stored before federation sealed inline credentials still hold
  // them as written, in every version. This asks the server what sealing them
  // would do (a dry run: nothing is written), then does it on the admin's say,
  // for exactly the agents the dry run listed. The server's order keeps a
  // running agent working: secrets first, a roll so the container holds them
  // (a stopped one's container is removed instead), and only then the configs.
  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  let report = $state<SealReport | null>(null)
  let busy = $state(false)
  let err = $state<string | null>(null)
  let dropUndeliverable = $state(false)

  const run = async (dryRun: boolean) => {
    busy = true
    err = null
    // An apply seals what the dry run listed, not whatever is sealable by the
    // time the button is pressed: an agent that became sealable since (and
    // might restart) waits for the next dry run.
    const agentIds = dryRun ? undefined : planned.map((a) => a.agentId)
    try {
      const j = await postJson<{ result?: SealReport }>('/api/fleet/seal-configs', { dryRun, dropUndeliverable, agentIds })
      if (!j.result) {
        err = 'the server sent no report'
        return
      }
      if (dryRun || report === null) {
        report = j.result
      } else {
        // The apply answers for the agents it was given; every other row
        // keeps what the dry run said about it.
        const done = new Map(j.result.agents.map((a) => [a.agentId, a]))
        report = { ...j.result, agents: report.agents.map((a) => done.get(a.agentId) ?? a) }
      }
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
  const removals = $derived(planned.filter((a) => a.removeStopped).length)
  const anyUndeliverable = $derived(shown.some((a) => a.undeliverable.length > 0))
  const applied = $derived(report !== null && !report.dryRun)

  const STATUS: Record<AgentSeal['status'], string> = {
    clean: 'Clean',
    planned: 'To seal',
    sealed: 'Sealed',
    skipped: 'Skipped',
  }
  const TONE: Record<AgentSeal['status'], ChipTone> = { clean: 'neutral', planned: 'neutral', sealed: 'accent', skipped: 'warn' }
</script>

<Modal open onClose={onClose} title="Seal credentials in stored configs" width="max-w-lg">
  <AutoHeight>
    <div class="space-y-4">
      <p class="font-sans text-[13px] leading-5 text-muted">
        Agents federated earlier still store credentials as written in every version of their config. Sealing turns each
        one into a <code class="font-mono text-xs text-fg">CONFIG_</code> secret of that agent. The value it runs on does not change.
        A running agent restarts once, and a stopped one gets a new container the next time it starts.
      </p>

      {#if report === null && busy}
        <p class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><WaitingMark site="fleet/seal-configs" size={12} /> Reading stored configs</p>
      {/if}

      {#if report}
        <div class="font-sans text-sm leading-5 text-fg tabular-nums">
          {#if applied}
            {shown.filter((a) => a.status === 'sealed').length} sealed · {shown.filter((a) => a.status === 'skipped').length} skipped · {clean} already clean
          {:else if shown.length === 0}
            Every stored config is clean: nothing to seal.
          {:else}
            {planned.length} to seal{rolls ? ` (${rolls} restart${rolls === 1 ? '' : 's'})` : ''}{removals
              ? ` (${removals} stopped container${removals === 1 ? '' : 's'} to remove)`
              : ''} · {shown.length - planned.length} skipped · {clean} already clean
          {/if}
        </div>
        {#if shown.length > 0}
          <ul use:listStagger class="max-h-72 overflow-y-auto rounded-lg border border-line [&>*+*]:border-t [&>*+*]:border-line-subtle">
            {#each shown as a (a.agentId)}
              <li class="space-y-1 px-4 py-2.5">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 truncate font-mono text-xs text-fg">{a.slug}</span>
                  {#if a.roll}
                    <span class="font-sans text-[13px] leading-5 text-muted">{a.rolled ? 'Restarted' : 'Restarts'}</span>
                  {:else if a.removeStopped}
                    <span class="font-sans text-[13px] leading-5 text-muted">{a.removed ? 'Container removed' : 'Stopped'}</span>
                  {/if}
                  <span class="flex-1"></span>
                  <Chip tone={TONE[a.status]}>{STATUS[a.status]}</Chip>
                </div>
                {#if a.reason}<p class="font-sans text-[13px] leading-5 text-warning">{a.reason}</p>{/if}
                {#if a.sealed.length}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    Secrets: <span class="font-mono text-xs text-fg">{a.sealed.join(', ')}</span>
                  </p>
                {/if}
                {#if a.versions.length}
                  <p class="font-sans text-[13px] leading-5 text-muted">Versions rewritten: {a.versions.map((v) => `v${v}`).join(', ')}</p>
                {/if}
                {#if a.changed.length}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    Older versions held a different value at <span class="font-mono text-xs text-fg">{a.changed.join('; ')}</span>: they now name the
                    current one.
                  </p>
                {/if}
                {#if a.historyOnly.length}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    Only older versions held <span class="font-mono text-xs text-fg">{a.historyOnly.join('; ')}</span>: the value is not kept, and
                    reverting to them leaves that setting out.
                  </p>
                {/if}
                {#if a.dropped.length}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    Removed, Talaria supplies these itself: <span class="font-mono text-xs text-fg">{a.dropped.join('; ')}</span>
                  </p>
                {/if}
                {#if a.undeliverable.length && a.status !== 'skipped'}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    Removed, too long or multi-line to seal: <span class="font-mono text-xs text-fg">{a.undeliverable.join('; ')}</span>
                  </p>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        {#each report.warnings as w (w)}
          <p class="font-sans text-[13px] leading-5 text-warning">{w}</p>
        {/each}
        {#if anyUndeliverable && !applied}
          <Checkbox
            checked={dropUndeliverable}
            disabled={busy}
            onChange={(on) => (dropUndeliverable = on)}
            label="Take out credentials that cannot be sealed (multi-line or over 8 KB). That setting becomes unconfigured."
          />
        {/if}
      {/if}
      {#if err}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{err}</p>{/if}
    </div>
  </AutoHeight>

  {#snippet footer()}
    <div class="flex justify-end gap-2">
      {#if applied}
        <Button variant="outline" size="sm" onclick={onClose}>Done</Button>
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
