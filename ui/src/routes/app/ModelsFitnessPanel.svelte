<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import FitnessDetail from '@/components/models/FitnessDetail.svelte'
  import FitnessMatrix from '@/components/models/FitnessMatrix.svelte'
  import FitnessRunModal from '@/components/models/FitnessRunModal.svelte'
  import { TIER_META, usd } from '@/components/models/fitness'
  import { useFitnessDetail, useModelFitness } from '@/components/models/fitness-queries'

  // ── Model fitness — "can I swap this model in", per role, from the UI ───────
  //
  // The whole point of the harness layer: Talaria has to work on a 7-14B
  // self-host and excel on a frontier model, and an admin has to be able to
  // tell WHICH, per role, without a week of production surprises. The matrix is
  // the answer at a glance; the run fills it; the drill-down is what makes it
  // trustworthy.
  const qc = useQueryClient()
  const query = useModelFitness()
  const data = $derived(query.data)

  // The selected model lives in the URL (UI-CONVENTIONS: any selection worth
  // sharing or revisiting is a search param, not component state) so an admin
  // can send a colleague the exact verdict they are arguing about.
  // sv-router auto-parses query values (numbers, bare flags). A model id must
  // stay a string — `gpt-4` is fine but a bare numeric id would arrive as a
  // number and never match a row.
  const selected = $derived.by((): string | null => {
    const v = searchParams.get('model')
    return v == null || v === true ? null : String(v)
  })
  const select = (model: string) => searchParams.set('model', model)
  const detailQuery = useFitnessDetail(() => selected)

  let running = $state(false)
  let candidate = $state('')
  const status = $derived(data?.status)
  const inFlight = $derived(status?.state === 'running')
  const row = $derived(data?.models.find((m) => m.id === selected))

  const openRun = (model: string) => {
    candidate = model
    running = true
  }
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['model-fitness'] })
    await qc.invalidateQueries({ queryKey: ['model-capabilities'] })
    await qc.invalidateQueries({ queryKey: ['model-fitness-detail'] })
  }
  const post = async (body: unknown) => {
    await fetch('/api/admin/model-fitness', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    await refresh()
  }
  const forget = async (model: string) => {
    // Audit 1.2's release valve. Destructive enough to confirm: it drops probe
    // results an admin paid for, not just the gateway's learned guesses.
    const ok = await confirm({
      title: 'Forget what we know about this model',
      message: `Every recorded capability for ${model} is deleted — probe results, what the gateway learned from a rejected parameter, and anything declared. Talaria keeps running it; it just stops claiming to know what it can do. Use this when a model id has been re-pointed at different weights.`,
      confirmLabel: 'Forget',
      danger: true,
    })
    if (ok) await post({ action: 'forget', model })
  }
</script>

{#if !data}
  <Panel>
    {#if query.isError}
      <QueryError
        variant="compact"
        error={query.error}
        title="Could not load model fitness"
        onRetry={() => void query.refetch()}
      />
    {:else}
      <Skeleton class="mb-4 h-4 w-32 rounded-full" />
      <SkeletonRows rows={5} />
    {/if}
  </Panel>
{:else}
  <div class="space-y-4">
    <Panel>
      <SectionHeader
        title="Model fitness"
        info="Rows are the models on your gateway, columns are the slots you can assign one to. A cell says what the last test found for that pairing. Grey means nothing has measured it — which is not a pass."
        action={`${data.registry.harnesses} harnesses · ${data.registry.fixtures} fixtures`}
      />

      <!-- The run strip: the ReindexStatus grammar (retrieval), because
           Talaria has one long-run mechanism and this follows it. -->
      <div class="mb-4 flex flex-wrap items-center gap-3 rounded-md border border-line p-3">
        {#if inFlight}
          <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            <GeneratingBars bars={3} variant="breathe" step={0.2} />
            {status?.model} · {status?.phase === 'scoring' ? 'scoring' : status?.phase ? TIER_META[status.phase].label : ''}
            {#if status && status.total > 0}
              · {status.done}/{status.total} fixtures{status.harness ? ` · ${status.harness}` : ''}
            {/if}
          </span>
          <span class="ml-auto"></span>
          <!-- Stop is honored at a case boundary and the sweep RESUMES: an
               admin who stops a run does not lose the calls already paid for. -->
          <Button size="sm" variant="outline" onclick={() => void post({ action: 'stop' })}>Stop</Button>
        {:else}
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            {#if status?.state === 'error'}
              <span class="text-danger">last run failed: {status.error}</span>
            {:else if status?.state === 'done' && status.model}
              last run: {status.model} · {status.tiers.join(' + ')}
            {:else}
              no run yet
            {/if}
          </span>
          <span class="ml-auto"></span>
          <Button size="sm" onclick={() => openRun(selected ?? data.models[0]?.id ?? '')} disabled={data.models.length === 0}>
            Test a model
          </Button>
        {/if}
      </div>

      {#if data.models.length === 0}
        <EmptyState icon="▤" title="No models registered yet" hint="Add a provider on the Models tab, then come back and test one." />
      {:else}
        <FitnessMatrix slots={data.slots} models={data.models} index={data.index} {selected} onSelect={select} />
        {#if data.registry.unfixtured.length > 0}
          <p class="mt-2 max-w-prose font-sans text-xs text-muted">
            {data.registry.unfixtured.length} registered harness(es) declare no eval fixtures, so no run can say anything about them. They are
            invisible to this matrix rather than passing it.
          </p>
        {/if}
      {/if}
    </Panel>

  </div>

  <!-- THE REPORT IS A DIALOG, not a panel under the matrix.
       It used to expand below, which put a full report — probes, per-slot
       verdicts, the tested-vs-observed table, the adversarial breakdown — at the
       bottom of a page whose top is a 21-column table. Clicking a cell scrolled
       nothing, so the answer to "what did this model do" appeared off-screen.
       A takeover modal puts it where the click was and gives it the height it
       needs; the selection stays in the URL either way, so a link to a specific
       model still opens straight onto its report. -->
  {#if selected}
    <Modal open={true} onClose={() => searchParams.delete('model')} takeover>
      {#snippet title()}
        <span class="flex flex-wrap items-center gap-2">
          <Chip tone="accent">{selected}</Chip>
          {#if data.index[selected]}
            <span class="font-mono text-[11px] normal-case tracking-normal text-muted">
              {data.index[selected]?.calls} call(s) · {usd(data.index[selected]?.costUsd ?? null)}
              {#if data.index[selected]?.partial} · partial run{/if}
            </span>
          {:else}
            <span class="font-mono text-[11px] normal-case tracking-normal text-muted">never tested</span>
          {/if}
        </span>
      {/snippet}

      {#if detailQuery.isPending}
        <SkeletonRows rows={6} />
      {:else if !detailQuery.data}
        <QueryError
          variant="compact"
          error={detailQuery.error}
          title="Could not load this model's report"
          onRetry={() => void detailQuery.refetch()}
        />
      {:else}
        <FitnessDetail detail={detailQuery.data} {row} />
      {/if}

      {#snippet footer()}
        <!-- Forget sits beside Test, not at the bottom of a scroll: it is the
             destructive twin of the button next to it and an admin should see
             both without hunting. -->
        <div class="flex flex-wrap items-center gap-2">
          <DangerLink onClick={() => void forget(selected)}>Forget recorded capabilities</DangerLink>
          <span class="ml-auto"></span>
          <Button size="sm" variant="outline" onclick={() => openRun(selected)} disabled={inFlight}>Test this model</Button>
          <Button size="sm" variant="ghost" onclick={() => searchParams.delete('model')}>Close</Button>
        </div>
      {/snippet}
    </Modal>
  {/if}

  {#if running}
    <FitnessRunModal
      open={running}
      models={data.models}
      bind:model={candidate}
      onClose={() => (running = false)}
      onStarted={() => {
        running = false
        void refresh()
      }}
    />
  {/if}
{/if}
