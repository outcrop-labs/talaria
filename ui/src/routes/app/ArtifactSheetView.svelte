<script lang="ts">
  import { Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { parseGrid } from './artifacts'

  // See parseGrid in artifacts.ts for the sheet body contract (JSON string[][],
  // row 0 = header). Edit mode is an editable grid with add/delete row+column;
  // read mode renders a table. Autosaves (debounced).
  let { value, editable, onSave }: { value: string; editable: boolean; onSave: (body: string) => void } = $props()

  let grid = $state<string[][]>(parseGrid(value))
  let timer: ReturnType<typeof setTimeout> | null = null
  $effect(() => () => {
    if (timer) clearTimeout(timer)
  })
  const commit = (g: string[][]) => {
    grid = g
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => onSave(JSON.stringify(g)), 600)
  }
  const setCell = (r: number, c: number, v: string) => commit(grid.map((row, ri) => (ri === r ? row.map((cell, ci) => (ci === c ? v : cell)) : row)))
  const addRow = () => commit([...grid, grid[0]!.map(() => '')])
  const addCol = () => commit(grid.map((row, i) => [...row, i === 0 ? `Column ${String.fromCharCode(65 + row.length)}` : '']))
  const delRow = (r: number) => grid.length > 1 && commit(grid.filter((_, i) => i !== r))
  const delCol = (c: number) => grid[0]!.length > 1 && commit(grid.map((row) => row.filter((_, i) => i !== c)))

  const cols = $derived(grid[0]?.length ?? 0)
  const head = $derived(grid[0] ?? [])
  const bodyRows = $derived(grid.slice(1))
</script>

{#if !editable}
  <div class="min-w-0 flex-1 overflow-auto p-6">
    <table class="border-collapse font-sans text-sm">
      <thead>
        <!-- §8 table header: raised fill + mono uppercase column labels. -->
        <tr>{#each head as h, i (i)}<th class="border border-line bg-card px-3 py-1.5 text-left font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted">{h}</th>{/each}</tr>
      </thead>
      <tbody>
        {#each bodyRows as row, ri (ri)}
          <tr class="transition-colors hover:bg-card2">{#each row as cell, ci (ci)}<td class="border border-line px-3 py-1.5 text-fg">{cell}</td>{/each}</tr>
        {/each}
      </tbody>
    </table>
  </div>
{:else}
  <div class="min-w-0 flex-1 overflow-auto p-6">
    <table class="border-collapse font-sans text-sm">
      <thead>
        <tr>
          {#each grid[0]! as _, c (c)}
            <th class="p-0.5">
              <button type="button" onclick={() => delCol(c)} title="Delete column" class="w-full rounded text-[10px] text-muted transition-colors hover:text-danger">✕</button>
            </th>
          {/each}
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each grid as row, r (r)}
          <tr>
            {#each row as cell, c (c)}
              <td class="border border-line p-0">
                <input
                  value={cell}
                  oninput={(e) => setCell(r, c, e.currentTarget.value)}
                  class={cn('w-40 bg-transparent px-2 py-1.5 text-sm text-fg outline-none focus:bg-card', r === 0 && 'font-semibold')}
                  placeholder={r === 0 ? 'Header' : ''}
                />
              </td>
            {/each}
            <td class="pl-1">
              {#if r > 0}<button type="button" onclick={() => delRow(r)} title="Delete row" class="text-[10px] text-muted transition-colors hover:text-danger">✕</button>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="mt-3 flex gap-2">
      <Button variant="outline" size="sm" onclick={addRow}><Plus size={12} class="mr-1" /> Row</Button>
      <Button variant="outline" size="sm" onclick={addCol}><Plus size={12} class="mr-1" /> Column</Button>
      <span class="self-center font-mono text-[10px] tracking-[0.05em] text-muted">{grid.length - 1} rows · {cols} columns</span>
    </div>
  </div>
{/if}
