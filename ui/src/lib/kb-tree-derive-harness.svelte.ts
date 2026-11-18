// The regression harness for the knowledge-tree-state live crash: KbDocRow's
// `const expanded = $derived(kbTreeState.docExpanded(doc.id))`, as a class —
// which is what that line compiles to. No DOM exists in this suite, so this is
// the component's reactive half, in the exact shape Svelte's compiler emits,
// for the test that mounts it inside $effect.root (see
// knowledge-tree-state.derive.test.ts). Test plumbing, not a module worth
// exporting beyond that test.
import { kbTreeState } from './knowledge-tree-state.svelte'

export class KbTreeDeriveHarness {
  /** The row's derived expansion read — the crash site when hydrate wrote
   *  $state on its first call, before this branch's eager `hydrate()`. */
  expanded = $derived(kbTreeState.docExpanded('d-1'))
  /** The sidebar's template-expression read ({#if sectionExpanded(...)}) —
   *  same rule, same fix. */
  sectionOpen = $derived(kbTreeState.sectionExpanded('s-1'))
}


/** "Mount" the harness the way a component's deriveds run: inside an effect
 *  root, so the first $derived evaluation is a reactive context — the exact
 *  one that threw state_unsafe_mutation before the eager hydrate. */
export function mountHarness(): { row: KbTreeDeriveHarness; stop: () => void } {
  let row!: KbTreeDeriveHarness
  const stop = $effect.root(() => {
    row = new KbTreeDeriveHarness()
    // Read both deriveds now, inside the root: the crash was on this first
    // evaluation, not a later one.
    void row.expanded
    void row.sectionOpen
  })
  return { row, stop }
}
/** Reset + rearm, so the harness's first read is a hydrate, as on arrival. */
export { resetKbTreeState } from './knowledge-tree-state.svelte'