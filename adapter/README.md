# mission-control Hermes adapter (legacy Phase-1, M4)

> **Heads up: this documents a legacy Phase-1 lift artifact.** In Phase 1, part of the plan was to
> make Hermes a first-class framework *inside* mission-control. This adapter is that artifact. Talaria
> no longer runs mission-control, we lifted its capabilities (task queue, cost, activity) into
> Talaria's own Postgres/Redis, and the product is the app in [`ui/`](../ui). This adapter is kept for
> reference (and as something you could still contribute upstream to mission-control), but it is not
> part of how Talaria runs today. See the [top-level README](../README.md) for current status.

The **mission-control-side adapter** made Hermes a first-class framework inside
[`builderz-labs/mission-control`](https://github.com/builderz-labs/mission-control), alongside its
CrewAI / LangGraph / AutoGen / Claude-SDK / OpenClaw adapters.

**Status:** written + verified (2026-07-01). Standalone artifact; contribute upstream (PR) or vendor.

## What it adds

A `hermes` `FrameworkAdapter` (register / heartbeat / reportTask / getAssignments / disconnect) that
fans lifecycle events onto mission-control's `eventBus` and reads assignments from the shared task
queue, mirroring the existing adapters. This makes Hermes appear in `GET /api/frameworks`, in the
`FRAMEWORK_REGISTRY` (with setup hints citing this Talaria repo), and in the universal agent
templates. Runtime behavior (register/heartbeat/report over REST) already worked via the Talaria
plugin without this; the adapter made Hermes *native* in mission-control's framework model.

## The change (3 files, +94 lines)

- **`src/lib/adapters/hermes.ts`** (new), the `HermesAdapter` (see [`hermes.ts`](./hermes.ts)).
- **`src/lib/adapters/index.ts`**, import + register `hermes: () => new HermesAdapter()`.
- **`src/lib/framework-templates.ts`**, a `hermes` `FRAMEWORK_REGISTRY` entry + `'hermes'` added to
  each universal template's `frameworks` list (so `getTemplatesForFramework('hermes')` returns
  templates and the adapter-loop test stays green).

The full diff is in [`mission-control-hermes-adapter.patch`](./mission-control-hermes-adapter.patch).
Apply against a mission-control checkout with:

```bash
git -C <mission-control> apply /path/to/mission-control-hermes-adapter.patch
```

## Verification

Built into the Phase-1 `talaria/stack` mission-control image (`talaria/vendor/mission-control`, pinned
at `d09e608` + this patch) and confirmed: `GET /api/frameworks` lists `hermes`, and
`getTemplatesForFramework('hermes')` resolves the universal templates. See
[`../docs/m0-contract.md`](../docs/m0-contract.md) and [`../scripts/verify-stack.sh`](../scripts/verify-stack.sh).

## Design note

Talaria never forces mission-control's **Aegis-gated `done`** transition. Hermes agents report toward
`quality_review` and completion flows through mission-control's own (human) approval. The adapter
preserves that; it only broadcasts lifecycle/task events, it does not auto-complete. (Same
human-in-the-loop guardrail Talaria keeps today: agents create and triage, but a human owns `done`.)
