// THE VERDICT LAYER — port of ui/src/server/fitness/score.ts.
//
// This file opens with the one type the tiers share, so the adversarial tier
// and the sweep can derive their bands from the same words the matrix renders.
// The rest of score.ts — the weighted roll-up, the per-model observed value,
// the arming decision — crosses with this slice's next commit, in dependency
// order underneath the tiers it reads.

/// The three words the fitness matrix renders, plus the two a cell can carry
/// before anything has been measured at it. Kept as ONE enum across every
/// producer so a band an admin recognizes cannot arrive at the UI under a key
/// the renderer has no entry for — tier 3 once said `not-a-fit` where every
/// other surface said `unfit`, and the difference rendered as raw text with
/// its own inline colour table.
///
/// Serializes lowercase exactly as the TS union spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitnessBand {
    Ready,
    Workable,
    Unfit,
    /// Never tested — the honest default for a model nobody has swept.
    Untested,
    /// No sweep binds this model to a capability key, so nothing about it can
    /// be said.
    Unbound,
}
