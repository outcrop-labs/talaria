- **A roll's two edges are wired at boot, and a roll that fails says so.** The
  reconcile crate reaches the renderer through two `OnceLock` edges
  (`RENDER_FLEET`, `NEXT_FREE_PORT`), and the crate split left both unset:
  `roll_agent` returned `next_free_port not wired` before it rendered
  anything, the control route discarded that verdict while answering
  `{"rolling": true}`, and a roll — the only path that makes a re-rendered
  config live without downtime — silently changed nothing while the agent kept
  running the config the operator had just replaced. `register_all` now wires
  both edges beside the fleet edges it already owns; the control route logs and
  audits the failure it used to drop (`agent.roll_failed`, the shape
  mcp-apply's queue already writes); and the boot test that pins the job table
  pins these two edges the same way, because an unwired edge has no other
  reader.
  Verified: `cargo test -p talaria-jobs` — with the wiring removed the boot
  test fails on `the roll's overlay renderer fell out of the boot wiring`, and
  passes with it. The failure was reproduced on a live deployment first (roll →
  `{"ok":true,"rolling":true}`, no render write, no new container, no log
  line).
