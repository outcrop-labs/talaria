// THE DEFS — port of harness/defs/*.ts, one Rust module per TS file. Each is
// a prompt, an output contract, and what to do when the contract holds or
// breaks, declared through `define.rs` and honored by nobody but `run.rs`.
//
// REGISTERING IS THE LAST STEP OF A PORT, not a follow-up: a def that is not
// in the registry is invisible in the two ways that matter most (the fitness
// suite cannot replay its fixtures; the admin panel cannot show its floor).
// The registry itself crosses once the defs are here to fill it.

pub mod summarizer;
pub mod titler;
