<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import ProviderMark from '@/components/fleet/ProviderMark.svelte'
  import { cn } from '@/lib/cn'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsEndpointModal from './ModelsEndpointModal.svelte'

  // Compact provider card — identity + a model count + Manage. Everything
  // detailed (the model list, pricing, privacy) lives in the modal so the main
  // view stays scannable.
  let { ep }: { ep: LlmEndpoint } = $props()

  let managing = $state(false)
</script>

<Panel class="flex items-center gap-3">
  <ProviderMark provider={ep.provider} name={ep.name} />
  <div class="min-w-0 flex-1">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-sm font-semibold text-fg">{ep.name}</span>
      <span
        class={cn(
          'shrink-0 font-mono text-[10px] uppercase tracking-[0.05em]',
          ep.class === 'local' ? 'text-success' : 'text-accent',
        )}
      >
        {ep.class === 'local' ? 'self-hosted' : 'cloud'}
      </span>
    </div>
    <div class="truncate font-mono text-[11px] text-muted">
      {ep.models.length} model{ep.models.length === 1 ? '' : 's'}{ep.baseUrl ? ` · ${ep.baseUrl}` : ''}
    </div>
  </div>
  <Button variant="outline" size="sm" class="shrink-0" onclick={() => (managing = true)}>
    Manage
  </Button>
</Panel>
{#if managing}
  <ModelsEndpointModal {ep} onClose={() => (managing = false)} />
{/if}
