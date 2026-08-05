# React → Svelte 5 migration guide (Talaria UI)

The stack after this migration: **Svelte 5 (runes)** + **sv-router** (code-based
tree in `src/router.ts`) + **@tanstack/svelte-query v6** + Tailwind v4 (styles
untouched). The API layer is framework-free (`defineApi`, already converted).
This guide is the contract for converting a `.tsx` file. Follow the exemplars:

- `src/components/ui/Button.svelte`, `Input.svelte`, `CloseButton.svelte` — leaf controls, rest-prop spread, `$bindable` refs
- `src/components/ui/Modal.svelte` — snippets, portal, transitions
- `src/components/ui/ConfirmHost.svelte` + `confirm.svelte.ts` — imperative API over module `$state`
- `src/components/ui/ErrorFallback.svelte` + `error.ts` — split of component/helpers, `$derived`
- `src/routes/api/agents.ts` — API route shape (already done for all of `routes/api/`)

## House rules

1. **Port comments.** Every comment that is still true moves with the code it
   describes. They are the codebase's memory; deleting them is a regression.
2. **Same look, same classes.** Tailwind class strings copy over verbatim
   (`className` → `class`). Do not redesign anything.
3. **Same file layout.** `foo-bar.tsx` → `FooBar.svelte` in the same directory.
   A file exporting several components splits into one `.svelte` per component
   (PascalCase of the export name) plus, if needed, a plain `.ts` for shared
   helpers/types/constants (see `button.ts`, `error.ts`, `control.ts`).
   Pure-TS exports stay in `.ts` files. Update every importer.
4. **Imports of Svelte components carry the `.svelte` extension.**
5. Old `.tsx` files are **deleted** in the same change that replaces them.

## Core translations

| React | Svelte 5 |
|---|---|
| `function C({ a, b = 1 }: Props)` | `let { a, b = 1 }: Props = $props()` |
| `className` | `class` (prop name: `class: className` in `$props()` destructure) |
| `children: ReactNode` | `children: Snippet`, render with `{@render children?.()}` |
| prop taking JSX (`title?: ReactNode`) | `title?: string \| Snippet` — `{#if typeof title === 'string'}` … else `{@render title()}` |
| `useState(x)` | `let v = $state(x)` |
| `useMemo(() => e, deps)` / derived values | `const v = $derived(e)` or `$derived.by(() => …)` |
| `useEffect(fn, deps)` | `$effect(() => { …; return cleanup })` — deps are automatic |
| `useRef<HTMLX>(null)` + `ref={r}` | `let el = $state<HTMLX \| null>(null)` + `bind:this={el}`; for component refs expose `ref = $bindable(null)` |
| `useCallback` | plain function |
| `memo`, `forwardRef`, `displayName` | delete — not needed |
| `onClick` / `onKeyDown` / `onFocus`… | `onclick` / `onkeydown` / `onfocus` (lowercase) |
| `onChange` on inputs | `oninput` (React's onChange fires per keystroke) — or better, `bind:value` |
| conditional render `{x && <A/>}` | `{#if x}<A/>{/if}` |
| `list.map(x => <A key={x.id}/>)` | `{#each list as x (x.id)}<A/>{/each}` |
| `dangerouslySetInnerHTML` | `{@html …}` |
| `createPortal(n, document.body)` | `use:portal` (see `@/lib/portal.ts`) |
| `createContext` / `useContext` | `setContext(KEY, …)` / `getContext(KEY)` — export the typed key + a `useX()` accessor from a small `.ts` module |
| `<ErrorBoundary what="X">…` | `<svelte:boundary>…{#snippet failed(error, reset)}<ErrorFallback {error} {reset} what="X" />{/snippet}</svelte:boundary>` |
| custom hook `useThing()` in a module | function in a `thing.svelte.ts` module using runes (yes, runes work in `.svelte.ts`) — returned state must be read via getters or returned objects' properties to stay reactive |

**Reactivity trap:** never destructure reactive sources. `const { data } =
query` freezes; use `query.data` (in templates or inside `$derived`).

## TanStack Query v5 → v6 (svelte)

```ts
// before (react)
const { data, isLoading } = useQuery({ queryKey: ['agents'], queryFn: fetchAgents })
// after (svelte) — options WRAPPED IN A FUNCTION, no destructuring
const query = createQuery(() => ({ queryKey: ['agents'], queryFn: fetchAgents }))
// use query.data, query.isLoading, query.isSuccess, query.error
```

- `useQuery`/`useMutation`/`useInfiniteQuery` → `createQuery`/`createMutation`/`createInfiniteQuery` from `@tanstack/svelte-query`
- `useQueryClient()` → `useQueryClient()` (unchanged name; call during component init, not in event handlers)
- `mutation.mutate(vars)` / `mutateAsync` unchanged; `mutation.isPending` unchanged
- Reactive keys work automatically because options are a function re-run on rune changes: `createQuery(() => ({ queryKey: ['board', boardId], … }))`

## Router (TanStack Router → sv-router)

All router APIs come from `@/router` (the `createRouter` result) or `sv-router`:

| Before | After |
|---|---|
| `<Link to="/boards/$boardId" params={{boardId}}>` | `<a href={p('/boards/:boardId', { params: { boardId } })}>` |
| active-link styling via `activeProps` | `{@attach isActiveLink()}` (from `sv-router`) or `isActive('/x')` |
| `useNavigate()(…)` / `navigate({ to })` | `navigate('/path')` or `navigate('/post/:slug', { params, search, replace })` |
| `Route.useParams()` / `useParams()` | `route.getParams('/boards/:boardId')` (strict) or `route.params` (loose) |
| `useRouterState({ select: s => s.location.pathname })` | `route.pathname` (reactive) |
| `useSearch()` / search params | `searchParams` from `sv-router` (reactive `URLSearchParams` that writes the URL) |
| `router.navigate({ to: '/', replace: true })` | `navigate('/', { replace: true })` |
| `<Outlet />` in a layout | `{@render children()}` (layouts receive `children: Snippet`) |
| `redirect()` thrown in `beforeLoad` | `throw navigate('/login')` inside the tree's `hooks.beforeLoad` |

Route components live under `src/routes/` (see the mapping comment in
`src/router.ts`). Page-level data loading stays in the component via
svelte-query (same as the React version did).

## Icons

`lucide-react` → `@lucide/svelte`. Same icon names, same `size`/`strokeWidth`
props. `import { X, Settings } from '@lucide/svelte'`.
An icon passed as a prop/value (`icon: LucideIcon`) types as
`import type { Icon as IconType } from '@lucide/svelte'` and renders via
`<const.icon />` → `{@const Icon = item.icon}<Icon size={16} />`.

## Motion (framer-motion → svelte transitions)

framer-motion is GONE. Use `@/lib/motion` (reduced-motion-aware wrappers of
`svelte/transition` — `fade`, `fly`, `scale`, `slide`, plus house presets
`QUICK`, `POP`, `PANEL`):

- `<AnimatePresence>{open && <motion.div …>}` → `{#if open}<div transition:fade={QUICK}>` (an `{#if}` block + `transition:`/`in:`/`out:` IS the AnimatePresence)
- Modals/popovers: `in:scale={POP}` `out:fade={QUICK}` (Modal.svelte does this — reuse Modal)
- Panels/drawers: `in:fly={PANEL}`
- Row/banner reveal: `transition:slide` (≤150ms) or `fade`
- Add default transitions where they make sense even if the React code had
  none: dropdown menus, popovers, toasts, banners, collapsible sections —
  small (≤180ms), calm, never springy. Do NOT animate route-level page swaps
  or large lists.
- `useReducedMotion` / `MotionConfig` → delete; the wrappers handle it.
- `motion.div` with `layout` / drag / spring physics: replicate the intent
  simply (CSS transitions or `animate:flip` with `{#each}`), note anything
  genuinely lost as a TODO comment.
- `svelte/motion` (`Tween`/`Spring`) only where a value animates continuously
  (e.g. progress numbers).

## Imperative dialogs

`import { confirm, alert, prompt } from '@/components/ui/confirm'` becomes
`from '@/components/ui/confirm.svelte'` (the `confirm.svelte.ts` module).
Call sites are unchanged (`await confirm({ … })`).

## Tiptap

`@tiptap/react` → `svelte-tiptap` (same tiptap v2 core + extensions, already in
package.json). `useEditor` → `createEditor`; `<EditorContent editor>` →
`<EditorContent editor={$editor} />` per svelte-tiptap's API (it returns a
readable store). `BubbleMenu`/`FloatingMenu` come from `svelte-tiptap` too.

## Markdown

`react-markdown` → build the HTML with unified directly (deps installed:
`unified`, `remark-parse`, `remark-gfm`, `remark-breaks`, `remark-rehype`,
`rehype-highlight`, `rehype-stringify`) and render with `{@html}` inside the
same wrapper classes. Custom component overrides (links opening externally,
code blocks) become rehype tweaks or post-render DOM handling — keep behavior.

## What does NOT change

- `src/server/**`, `src/routes/api/**` (already converted), `src/sdk/server.ts`
- Pure-TS `src/lib/*.ts` modules with no React imports
- `styles.css`, Tailwind classes, fonts, theme tokens
- Test files for pure logic (`*.test.ts`)

## Definition of done for a converted file

- No `react` / `react-dom` / `@tanstack/react-*` / `framer-motion` /
  `lucide-react` / `@tiptap/react` imports remain.
- The old `.tsx` file is deleted; all importers updated.
- `npx svelte-check` clean for the new files (or strictly fewer errors than
  before your change).
- Comments ported; visual classes byte-identical unless a transition was added.
