// The runs engine — port of ui/src/server/runs/** (batch 4). One module per
// TS file, dependency-ordered: the lease primitive everything stands on, the
// `runs` store (every write a CAS), the definitions/state machine, the driver
// (enqueue/drive/cancel), the awaiting decision path, and the reclaim sweep.
// The registered scheduler that drives reclaim and the run kinds land later
// in the batch; until the handoff slice, nothing here is armed.

pub mod define;
pub mod lease;
pub mod store;
