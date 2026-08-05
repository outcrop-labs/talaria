<script lang="ts">
  import { Sparkles } from '@lucide/svelte'
  import { navigate } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import AssistantWizard from '@/components/assistant/AssistantWizard.svelte'
  import { useAssistant } from '@/lib/assistant'

  // The personal assistant card: everyone can set up their own agent (its own
  // container, memory, key) through the onboarding wizard, then jump into chat.
  let wizard = $state(false)
  const query = useAssistant()
</script>

{#if query.isLoading}
  <Panel class="flex items-center gap-4">
    <Skeleton class="h-11 w-11 shrink-0 rounded-md" />
    <div class="min-w-0 flex-1 space-y-2">
      <Skeleton class="h-3 w-40 rounded-full" />
      <Skeleton class="h-2.5 w-64 rounded-full" />
    </div>
  </Panel>
{:else}
  <Panel class="flex items-center gap-4">
    <span class="grid h-11 w-11 shrink-0 place-items-center rounded-md border border-line-strong bg-raised text-accent">
      <Sparkles size={20} />
    </span>
    {#if query.data}
      <div class="min-w-0 flex-1">
        <div class="font-sans text-sm font-medium text-fg">{query.data.displayName}</div>
        <div class="truncate font-sans text-xs text-muted">Your personal assistant, with its own memory, skills, and tools.</div>
      </div>
      <Button size="sm" onclick={() => void navigate('/chat')}>
        Open chat
      </Button>
    {:else}
      <div class="min-w-0 flex-1">
        <div class="font-sans text-sm font-medium text-fg">Set up your assistant</div>
        <div class="truncate font-sans text-xs text-muted">A personal agent that's just yours: memory, skills, and tools of its own.</div>
      </div>
      <Button size="sm" onclick={() => (wizard = true)}>
        Get started
      </Button>
    {/if}
    {#if wizard}<AssistantWizard onClose={() => (wizard = false)} />{/if}
  </Panel>
{/if}
