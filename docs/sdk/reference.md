# SDK reference

Every export from both entry points, one row each. **This file is enforced**:
`bun run check` parses the exports out of `ui/src/sdk/index.ts` and
`ui/src/sdk/server.ts` and fails if a symbol here doesn't exist, or an export
is missing from here. Guides live beside this file; the row is the claim.

Import them as:

```ts
import { Button, useAppQuery } from '@talaria/sdk'          // client
import { defineAppServer, z } from '@talaria/sdk/server'    // server
```

## `@talaria/sdk` — app definition

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `defineApp` | fn | Declare the app's surfaces; default-export from app.ts |
| `AppSurfaces` | type | The surfaces map (work / manage / settings), all optional |

## `@talaria/sdk` — UI kit: actions

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `Button` | comp | The button. Variants: primary, outline, ghost, danger, danger-outline, accent-soft, link |
| `buttonClasses` | fn | Button styling for a non-button element that must read as one |
| `ButtonProps` | type | Props of `Button` |
| `IconButton` | comp | Icon-only button; sizes include the bordered tile |
| `CloseButton` | comp | The × affordance |
| `CopyButton` | comp | Copies text, flashes confirmation |
| `CopyLinkButton` | comp | Copies a link, flashes confirmation |
| `DangerLink` | comp | A link styled as a destructive action |
| `useSavedFlash` | fn | The saved…✓ flash cycle for your own save buttons |

## `@talaria/sdk` — UI kit: inputs

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `Input` / `InputProps` | comp / type | Text input |
| `Textarea` / `TextareaProps` | comp / type | Multi-line input |
| `Select` / `SelectProps` | comp / type | Native select, styled |
| `Combobox` / `ComboOption` | comp / type | Searchable pick-list |
| `Checkbox` | comp | Checkbox; a bare cell form for tables |
| `Radio` | comp | Radio button |
| `Toggle` | comp | On/off switch |
| `InlineCreate` | comp | Click-to-add row (the + Add thing pattern) |
| `submitOnEnter` | fn | Enter submits, Escape blurs — the inline-edit contract |
| `inlineEditKeys` | fn | Key handler for inline edit fields |
| `controlSizes` / `ControlSize` | const / type | The size scale shared by controls |
| `RichEditor` / `RichEditorHandle` | comp / type | WYSIWYG editor (markdown under the hood) |

## `@talaria/sdk` — UI kit: structure

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `Panel` / `PanelProps` | comp / type | The card surface; an as-prop for semantic elements |
| `SectionHeader` | comp | In-panel section title |
| `ViewHeader` | comp | Page-level header |
| `Tabs` / `TabItem` | comp / type | Tab strip |
| `Segmented` / `SegmentedOption` | comp / type | Segmented control |
| `Modal` | comp | Centered overlay dialog |
| `Disclosure` | comp | Expandable section |
| `StatCard` | comp | Metric tile; may wrap a link |
| `Steps` | comp | Numbered step indicator |

## `@talaria/sdk` — UI kit: display

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `Chip` / `ChipProps` / `ChipTone` | comp / types | Label pill; filter and removable-token forms |
| `StatusDot` / `DotStatus` | comp / type | Colored state dot |
| `EmptyState` | comp | The nothing-here message; full / compact / inline variants |
| `Avatar` | comp | User/agent avatar |
| `Kbd` | comp | Keyboard key glyph |
| `InfoTip` | comp | Hoverable explanation tip |
| `CodeBlock` | comp | Monospace block with copy |
| `Markdown` | comp | Render markdown as platform-styled content |
| `Popover` | comp | Anchored content panel; outside-click, Escape, scroll handled |

## `@talaria/sdk` — UI kit: loading + feedback

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `Skeleton` / `SkeletonRows` / `SkeletonCard` | comp | A fetch hasn't resolved |
| `Generating` / `GeneratingDots` / `GeneratingOverlay` | comp | Model output is being written |
| `Waiting` / `WaitingMark` | comp | An agent is working right now |
| `InlineWaitingSite` | type | Declare an app's own waiting site inline |
| `WaitingRole` / `WaitingSlot` | type | Waiting vocabulary for those sites |
| `confirm` / `alert` / `prompt` | fn | The platform's dialogs |

## `@talaria/sdk` — UI kit: menus

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `useContextMenu` | fn | Right-click menu; items via `ContextMenuItem` |
| `ContextMenuItem` / `ContextMenuEntry` | types | Menu row shapes |
| `DropdownMenu` | comp | Anchored menu with the item grammar |

## `@talaria/sdk` — motion + utilities

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `fade` / `fly` / `scale` / `slide` / `flip` | fn | svelte/transition wrappers, reduced-motion aware |
| `QUICK` / `POP` / `PANEL` / `LIST` | const | Platform duration/easing presets |
| `cn` | fn | Class combiner |

## `@talaria/sdk` — session + data

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `useMe` | fn | The session query; data is `SessionUser` or null |
| `useHasPerm` | fn | Permission check as a `{ current }` box, false until resolved |
| `useIsAdmin` | fn | Plain predicate over a user |
| `SessionUser` | type | The signed-in user's shape |
| `createQuery` / `createMutation` | fn | svelte-query primitives (options are functions in v6) |
| `useQueryClient` | fn | The query client from context |
| `keepPreviousData` | fn | Page-flip placeholder |
| `api` | fn | JSON fetch against any platform API; throws the server's error |
| `appApi` | fn | get/post/put/patch/del bound to this app's routes |
| `useAppQuery` | fn | svelte-query read of an app-server GET; path may be a getter |
| `useAppInvalidate` | fn | Invalidate this app's queries after a write |

## `@talaria/sdk/server` — app server

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `defineAppServer` | fn | Declare the fetch handler; default-export from server.ts |
| `AppServer` | type | What `defineAppServer` takes |
| `AppRequestContext` | type | ctx: user, app, path, url, store |
| `SessionUser` | type | The authenticated user the handler runs as |
| `json` | fn | JSON response helper (`json(body, { status })`) |
| `parseBody` | fn | Zod-validate a request body; returns data or the standard 400 |
| `z` | const | Zod, re-exported (apps can't install it) |
| `AppStore` | type | The per-app document store interface |
| `AppDoc` | type | A stored document: id, data, createdAt, updatedAt |

## `@talaria/sdk/server` — MCP tools

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `defineAppMcp` | fn | Declare agent-callable tools; default-export from mcp.ts |
| `AppMcp` | type | The tools map `defineAppMcp` takes |
| `AppMcpTool` | type | One tool: name, description, inputSchema, handler |
| `AppMcpContext` | type | Tool handler ctx: app, agent, store |

## `@talaria/sdk/server` — workbench harnesses

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `defineWorkbenchHarness` | fn | Declare a coding harness agents drive; default-export from harness.ts |
| `WorkbenchHarnessDefinition` | type | The declarative contract (auth, invoke, guide…) |
| `HarnessMcpRenderContext` | type | What a custom MCP config renderer receives |

## `@talaria/sdk/server` — activity harnesses

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `defineHarness` | fn | Declare a model call Talaria runs; default-export from harnesses/*.ts |
| `HarnessDefinition` | type | The full activity contract (render, output, model, evals…) |
| `RenderContext` | type | What the render step receives |
| `Message` | type | One chat message |
| `Grounding` | type | Retrieval grounding for a render |
| `ModelSpec` | type | The model chain spec |
| `Capability` | type | A capability a harness requires of a model |
| `RoleFloor` | type | The minimum-model declaration |
| `Verify` | type | Output verifier signature |
| `CheckResult` | type | What a check returns: null (pass), or the gap |
| `EvalCase` | type | One fixture: input, check |
| `EvalBand` | type | Fixture banding for the fitness matrix |
| `EvalContext` | type | What a fixture check receives |
| `belowAnswerFloor` | fn | The floor a one-sided text fixture needs |
| `NO_TOOLS` | const | The `EvalContext` when no tools ran — most fixtures |
| `ToolDefinition` | type | A tool offered on one turn (OpenAI wire shape) |
| `ToolCall` | type | The model's use of one |
| `ToolPolicy` | type | Whether a turn may use tools at all |

## `@talaria/sdk/server` — the runner (bridge pattern)

| symbol | kind | what it is |
| :--- | :--- | :--- |
| `runHarness` | fn | The one runner — resolve, floor, render, call, parse, repair, guard, meter |
| `resolveHarnessModel` | fn | Free probe: which chain would carry this harness, or null |
| `RunContext` | type | Caller + signal for a direct run |
| `RunLedger` | type | Where the run's cost lands |
| `HarnessResult` | type | What `runHarness` resolves to |
| `ModelChainStep` | type | Which link of the chain answered |
| `ResolvedHarnessModel` | type | The chain's answer: model + step |
