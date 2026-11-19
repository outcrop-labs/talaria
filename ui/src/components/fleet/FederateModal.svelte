<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import Steps from '@/components/ui/Steps.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import { reconcileFleet } from '@/lib/fleet-defs'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { listStagger, slide, staggerIn } from '@/lib/motion'

  interface FederateResult {
    agents: Array<{
      slug: string
      model: string
      status: 'federated' | 'exists'
      /** Per-agent secrets the config's inline credentials were sealed as. */
      sealed?: string[]
      /** Config paths of inline credentials Talaria supplies itself — not stored. */
      dropped?: string[]
    }>
    errors: string[]
  }

  const STEPS = ['Source', 'Federate', 'Start'] as const

  // Bring outside agents in — as natives. Reads a Hermes-format directory and
  // recreates each agent inside Talaria: our chassis, our orchestration, fresh
  // key and state volume, skills carried over. The source dir is not referenced
  // again afterwards.
  // onSealStored: hand over to the stored-config seal (configs federated
  // before inline credentials were sealed on the way in).
  let { onClose, onSealStored }: { onClose: () => void; onSealStored?: () => void } = $props()

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
      started = r.error ?? (r.pending ? 'Still starting. The roster shows each agent as it comes up.' : `Started ${r.started?.length ?? 0} · already running ${r.alreadyRunning?.length ?? 0}`)
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
            <p class="font-sans text-[13px] leading-5 text-muted">
              Each agent in a Hermes directory is recreated to run on Talaria, with a fresh key and its skills carried
              over. Agents whose handle already exists are skipped, and credentials written in a config are sealed as
              that agent's secrets.
            </p>
            <SettingsField label="Directory on the server" for="federate-dir" hint="It holds agents.yaml, and a SOUL.md and config.yaml for each agent.">
              <Input
                id="federate-dir"
                size="sm"
                class="font-mono"
                bind:value={dir}
                onkeydown={submitOnEnter(() => {
                  if (!busy && dir.trim()) void federate()
                })}
                placeholder="/path/to/stack"
                autofocus
              />
            </SettingsField>
            {#if err}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{err}</p>{/if}
            {#if onSealStored}
              <p class="font-sans text-[13px] leading-5 text-muted">
                Federated agents before credentials were sealed?
                <button type="button" class="text-accent underline-offset-2 hover:underline" onclick={onSealStored}>
                  Seal the credentials in their stored configs
                </button>
              </p>
            {/if}
          </div>
        {/if}

        {#if step === 1 && result}
          <div use:staggerIn class="space-y-3">
            <div class="font-sans text-sm leading-5 text-fg tabular-nums">
              {fresh.length} federated · {result.agents.length - fresh.length} already here
            </div>
            {#if result.agents.length > 0}
              <!-- data-no-stagger: the surrounding staggerIn cascade skips this
                   list so listStagger alone owns its rows (one cascade rule). -->
              <ul data-no-stagger use:listStagger class="max-h-64 overflow-y-auto rounded-lg border border-line [&>*+*]:border-t [&>*+*]:border-line-subtle">
                {#each result.agents as a (a.slug)}
                  <li class="px-4 py-2.5">
                    <div class="flex items-center gap-2">
                      <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{a.slug}</span>
                      <Chip tone={a.status === 'federated' ? 'accent' : 'neutral'}>{a.status === 'federated' ? 'Federated' : 'Already exists'}</Chip>
                    </div>
                    {#if a.sealed?.length}
                      <p class="mt-1 font-sans text-[13px] leading-5 text-muted">
                        Sealed as its secrets: <span class="font-mono text-xs text-fg">{a.sealed.join(', ')}</span>
                      </p>
                    {/if}
                    {#if a.dropped?.length}
                      <p class="mt-1 font-sans text-[13px] leading-5 text-muted">
                        Not stored, because Talaria supplies these itself: <span class="font-mono text-xs text-fg">{a.dropped.join(', ')}</span>
                      </p>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
            {#each result.errors as e (e)}
              <p class="font-sans text-[13px] leading-5 text-danger">{e}</p>
            {/each}
          </div>
        {/if}

        {#if step === 2}
          <div use:staggerIn><p class="font-sans text-sm leading-5 text-fg">{started}</p></div>
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
        <Button variant="outline" size="sm" onclick={onClose}>Done</Button>
      </div>
    {/if}
  {/snippet}
</Modal>
