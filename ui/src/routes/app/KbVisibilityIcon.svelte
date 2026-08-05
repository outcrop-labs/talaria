<script lang="ts">
  import { Globe, Lock, Users } from '@lucide/svelte'

  let { v }: { v: 'private' | 'org' | 'public' } = $props()
</script>

{#if v === 'public'}<Globe size={12} />{:else if v === 'private'}<Lock size={12} />{:else}<Users size={12} />{/if}
