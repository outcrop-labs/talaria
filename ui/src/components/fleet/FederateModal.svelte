<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import Steps from '@/components/ui/Steps.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import { reconcileFleet } from '@/lib/fleet-defs'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { listStagger, slide, staggerIn } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  interface FederateResult {
    agents: Array<{ slug: string; model: string; status: 'federated' | 'exists' }>
    errors: string[]
  }

  const STEPS = ['Source', 'Federate', 'Start'] as const

  // Bring outside agents in — as natives. Reads a Hermes-format directory and
  // recreates each agent inside Talaria: our chassis, our orchestration, fresh
  // key and state volume, skills carried over. The source dir is not referenced
  // again afterwards.
  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  let step = $state(0)
  let dir = $state('')
  let busy = $state(false)
  let result = $state<FederateResult | null>(null)
  let started = $state<string | null>(null)
  let err = $state<string | null>(null)

  const federate = async () => {
    busy = true
    err = null
    try {
      const j = await postJson<{ result?: FederateResult }>('/api/fleet/federate', { dir: dir.trim() })
      if (!j.result) {
        err = 'federation failed'
        return
      }
      result = j.result
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      step = 1
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }

  const startAll = async () => {
    busy = true
    try {
      const r = await reconcileFleet()
      started = r.error ?? `Started ${r.started?.length ?? 0} · already running ${r.alreadyRunning?.length ?? 0}`
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      step = 2
    } finally {
      busy = false
    }
  }

  const fresh = $derived(result?.agents.filter((a) => a.status === 'federated') ?? [])
</script>

<Modal open onClose={onClose} title="Federate outside agents" width="max-w-lg">
  <!-- Step-to-step motion (ANIMATIONS.md wizard row): AutoHeight glides the
       panel between step heights, {#key step} + staggerIn brings each step's
       sections in 40ms apart. No exit on the outgoing step — the incoming
       stagger + gliding height IS the transition. -->
  <AutoHeight>
    <div class="space-y-5">
      <Steps steps={STEPS} current={step} />

      {#key step}
        {#if step === 0}
          <div use:staggerIn class="space-y-4">
            <p class="text-sm leading-relaxed text-muted">
              Point at a Hermes-format directory on the server (<code class="text-fg">agents.yaml</code> roster,
              each agent's <code class="text-fg">SOUL.md</code> + <code class="text-fg">config.yaml</code>).
              Each agent is recreated <em>natively</em>: it runs on Talaria itself, with a fresh key and state
              volume, models mapped into the registry, and its skills carried over. The source directory is never referenced
              again. Agents whose handle already exists are skipped.
            </p>
            <div>
              <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Directory on server</label>
              <Input
                bind:value={dir}
                onkeydown={submitOnEnter(() => {
                  if (!busy && dir.trim()) void federate()
                })}
                placeholder="/path/to/stack"
                autofocus
              />
            </div>
            {#if err}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{err}</p>{/if}
          </div>
        {/if}

        {#if step === 1 && result}
          <div use:staggerIn class="space-y-3">
            <div class="text-sm text-fg">
              {fresh.length} federated · {result.agents.length - fresh.length} already here
            </div>
            {#if result.agents.length > 0}
              <!-- data-no-stagger: the surrounding staggerIn cascade skips this
                   list so listStagger alone owns its rows (one cascade rule). -->
              <ul data-no-stagger use:listStagger class="max-h-56 divide-y divide-line overflow-y-auto rounded-lg border border-line">
                {#each result.agents as a (a.slug)}
                  <li class="flex items-center gap-2 px-3.5 py-2 text-sm">
                    <span class="font-mono text-xs text-fg">{a.slug}</span>
                    <span class={cn('ml-auto font-mono text-[10px] uppercase tracking-[0.05em]', a.status === 'federated' ? 'text-accent' : 'text-muted')}>
                      {a.status === 'federated' ? 'federated' : 'already exists'}
                    </span>
                  </li>
                {/each}
              </ul>
            {/if}
            {#each result.errors as e (e)}
              <p class="text-xs text-danger">
                {e}
              </p>
            {/each}
          </div>
        {/if}

        {#if step === 2}
          <div use:staggerIn><p class="text-sm text-fg">{started}</p></div>
        {/if}
      {/key}
    </div>
  </AutoHeight>

  {#snippet footer()}
    {#if step === 0}
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onclick={() => void federate()} disabled={busy || !dir.trim()}>
          {#if busy}<WaitingMark site="fleet/federate" size={12} />{/if}
          {busy ? 'Federating' : 'Federate'}
        </Button>
      </div>
    {:else if step === 1}
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Close
        </Button>
        <Button size="sm" onclick={() => void startAll()} disabled={busy || fresh.length === 0}>
          {busy ? 'Starting' : `Start ${fresh.length} agent${fresh.length === 1 ? '' : 's'}`}
        </Button>
      </div>
    {:else}
      <div class="flex justify-end">
        <Button size="sm" onclick={onClose}>
          Done
        </Button>
      </div>
    {/if}
  {/snippet}
</Modal>
