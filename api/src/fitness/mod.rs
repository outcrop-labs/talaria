// The model-fitness plane: tier 1 probes, the tier 2 sweep, tier 3 adversarial
// corpus, the verdict, and the surface the admin page reads. Port of
// ui/src/server/fitness/* — the engine half of /api/admin/model-fitness.

pub mod adversarial;
pub mod code_runner;
pub mod evals;
pub mod health;
pub mod live_feed;
pub mod observed;
pub mod probes;
pub mod score;
pub mod surface;
pub mod toolbox;
pub mod transcripts;
pub mod value;

// The store half of surface.rs (the record, the pricing derivations, the
// endpoints' assembly) crosses with the routes that serve it, in this module's
// last commit.
