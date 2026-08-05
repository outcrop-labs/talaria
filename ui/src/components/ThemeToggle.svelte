<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { applyTheme, getStoredTheme, toggleVariant, type ThemeId } from '@/lib/theme'

  // Dark/light switch for the two Mercury modes. Reuses the Button primitive.
  let theme = $state<ThemeId>('mercury')

  $effect(() => {
    theme = getStoredTheme()
  })

  const flip = () => {
    const next = toggleVariant(theme)
    applyTheme(next)
    theme = next
  }

  const isDark = $derived(theme === 'mercury')
</script>

<Button
  variant="outline"
  size="sm"
  onclick={flip}
  aria-label={isDark ? 'Switch to light mode' : 'Switch to dark mode'}
  title={isDark ? 'Mercury Light' : 'Mercury'}
  class="w-9 px-0 text-base"
>
  {isDark ? '☾' : '☀'}
</Button>
