<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import { personTint } from '@/lib/person-tint'

  let { name, size = 'sm' }: { name: string; size?: 'sm' | 'md' | 'lg' } = $props()

  const tint = $derived(personTint(name))
</script>

<span class="contents" style:--person-fill="var(--inbox-tint-{tint})" style:--person-ink="var(--inbox-tint-{tint}-ink)">
  <Avatar {name} class="inbox-person inbox-person-{size}" />
</span>
