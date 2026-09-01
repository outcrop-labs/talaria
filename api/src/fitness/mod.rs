// The model-fitness plane: tier 1 probes, the tier 2 sweep, tier 3 adversarial
// corpus, the verdict, and the surface the admin page reads. Port of
// ui/src/server/fitness/* — the engine half of /api/admin/model-fitness.

pub mod toolbox;

// The sweep (evals), probes, adversarial corpus, score, observed, value,
// health, live feed, transcripts and surface cross with this slice's later
// commits, in dependency order underneath this module.
