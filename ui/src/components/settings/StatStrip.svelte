<script lang="ts">
  import type { Snippet } from 'svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { cn } from '@/lib/cn'

  // One card of stats side by side, divided by hairlines. Children are `Stat`s,
  // any number from one to eight. `columns` is the most that share a row.
  let {
    columns = 4,
    class: className,
    children,
  }: { columns?: 2 | 3 | 4; class?: string; children: Snippet } = $props()

  // How many stats share a row, read from how many there are. Fewer than
  // `columns` split the card evenly, and a count that would leave one stat
  // alone takes a narrower row instead (five and six sit three to a row).
  // The last stat grows across whatever its row has left, so every hairline
  // stays on the same fraction as the row above and no cell is empty.
  const PER_ROW = {
    2: 'sm:[--per:2] sm:has-[>:nth-child(1):last-child]:[--per:1]',
    3: 'sm:[--per:3] sm:has-[>:nth-child(1):last-child]:[--per:1] sm:has-[>:nth-child(2):last-child]:[--per:2] sm:has-[>:nth-child(4):last-child]:[--per:2]',
    4: 'sm:[--per:2] sm:has-[>:nth-child(1):last-child]:[--per:1] lg:[--per:4] lg:has-[>:nth-child(2):last-child]:[--per:2] lg:has-[>:nth-child(3):last-child]:[--per:3] lg:has-[>:nth-child(5):last-child]:[--per:3] lg:has-[>:nth-child(6):last-child]:[--per:3]',
  }
</script>

<Panel class={cn('overflow-hidden p-0', className)}>
  <!-- Each stat draws its own right and bottom hairline; the row runs one
       pixel past the card's right and bottom edge so the outer ones are cut. -->
  <div
    class={cn(
      '-mb-px -mr-px flex flex-wrap [--per:1] [&>*]:shrink-0 [&>*]:grow-0 [&>*]:basis-[calc(100%/var(--per))] [&>*:last-child]:grow',
      PER_ROW[columns],
    )}
  >
    {@render children()}
  </div>
</Panel>
