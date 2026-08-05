<script lang="ts">
  // Thin naming wrapper over the one Drawer primitive: FocusInbox mounts one
  // of these per drawer ({#if drawer}), so it is always open while it exists.
  // Shell, backdrop, Escape, portal, and motion all come from Drawer.
  import type { Snippet } from 'svelte'
  import Drawer from '@/components/ui/Drawer.svelte'

  let { title, onClose, children }: { title: string; onClose: () => void; children: Snippet } = $props()
</script>

<Drawer open {onClose} {title}>
  {@render children()}
</Drawer>
