// A plugin's harness columns in Models → Fitness: `app_harness_fitness`.
//
// The fitness matrix keeps one row per model (`model_fitness_index`), and an
// app harness's column is `app:<slug>/<harness id>`. This reads those
// columns for one plugin — the band and its reason per model — and, for the
// models swept most recently, the harness's own scores from that model's
// full report (`model_fitness_report:<model>`). Every part is a read.

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_gateway::settings::get_setting;

use crate::Plugin;
use crate::status::harness_report;
use crate::upkeep::app_columns;

/// How many models' full reports one answer reads. Each is one settings row
/// of up to a few hundred kilobytes.
pub const FITNESS_MODEL_LIMIT: usize = 12;

/// The scores a harness column carries from a model's report, by harness id.
fn harness_scores(record: &Value, harness: &str) -> Value {
    record["harnesses"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|h| h["id"] == harness)
        .map(|h| {
            json!({
                "cases": h["cases"],
                "skipped": h["skipped"],
                "taskScore": h["taskScore"],
                "bandScores": h["bandScores"],
                "contractRate": h["contractRate"],
                "repairRate": h["repairRate"],
                "guardRate": h["guardRate"],
                "latencyP50": h["latencyP50"],
                "skipReason": h["skipReason"],
            })
        })
        .unwrap_or(Value::Null)
}

/// The whole `app_harness_fitness` answer.
pub async fn harness_fitness(pg: &PgPool, p: &Plugin) -> Value {
    let index = get_setting(pg, "model_fitness_index", json!({})).await;
    let columns = app_columns(&index, &p.app_slug);
    let mut models = Vec::new();
    for (i, (model, cols)) in columns.iter().enumerate() {
        let record = if i < FITNESS_MODEL_LIMIT {
            get_setting(pg, &format!("model_fitness_report:{model}"), Value::Null).await
        } else {
            Value::Null
        };
        models.push(json!({
            "model": model,
            "at": index[model]["at"],
            "columns": cols.iter().map(|(harness, (band, reason))| json!({
                "harness": harness,
                "band": band,
                "reason": reason,
                "scores": harness_scores(&record, harness),
            })).collect::<Vec<_>>(),
        }));
    }
    json!({
        "slug": p.app_slug,
        "harnesses": harness_report(&p.app_slug),
        "models": models,
        "note": if models.is_empty() {
            "No model has been swept with this app's harnesses yet: its columns appear in Models → Fitness once the app is enabled and an admin runs a sweep."
        } else {
            "Bands are ready, workable, unfit, untested or unbound. A regression since the last check files an eval-regression ticket on the plugin's board."
        },
    })
}
