// The update engine — the app rolls its own container.
//
// WHY THIS SHAPE. An update used to mean the orchestrator (compose/Dokploy)
// cutting the live container out and rebuilding it: minutes of downtime per
// deploy, and the in-app updater that could own this was a git-checkout TS
// relic, structurally off in containers. This module is the replacement: the
// Rust api drives docker through the socket it already mounts, rolling the
// app the way the fleet rolls agents (fleet/reconcile.rs is the precedent —
// render the incoming slot, gate on real health, cut over, drain, retire).
//
// THE ORDER OF TRUST, everywhere in here:
//   digest  the only ref a roll ever pulls. A moving tag half-pushed
//           mid-build is exactly what a roll must never boot, so the
//           registry resolves `main` → digest and everything downstream
//           (pulls, pins, state) names digests.
//   health  the compose healthcheck on the slot is the gate. An unhealthy
//           replacement never takes over; the old container keeps serving
//           and the run records why.
//   self    the orchestrating process IS the old container — `docker stop
//           <self>` kills the choreography. The roll splits: old does pull →
//           green up → healthy → alias → record cutting-over → stop self;
//           GREEN's boot reconcile finishes (state/mod's reconcile_boot).
//
// DORMANT BY DEFAULT. Nothing here acts until an instance is ADOPTED
// (adopt.rs, phase by phase): every existing install keeps deploying the
// way it always has, forever. The mode gate (mode.rs) is the first check in
// every verb: an install that isn't `image` mode gets a sentence, never a
// container.
pub mod docker;
pub mod layout;
pub mod mode;
pub mod registry;
pub mod render;
pub mod state;
