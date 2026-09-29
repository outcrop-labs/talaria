// Google Sheets, the agent-facing half: read a spreadsheet's cells, and write
// a rectangle of them back.
//
// WHY THE SHEETS API AND NOT DRIVE EXPORT. Drive can already hand us a whole
// spreadsheet as CSV (talaria-google-drive's import path does exactly that),
// and the symmetric write would be uploading a CSV back over the file. That
// write is destructive in a way a person would not forgive: it flattens every
// tab but the first, and it replaces formulas with their last computed values.
// The values API writes the rectangle it is given and leaves the rest of the
// document — other tabs, formatting, formulas outside the range — untouched.
//
// SCOPE. Both endpoints accept `https://www.googleapis.com/auth/drive`, which
// every Talaria connection already carries (talaria-google-oauth's
// WORKSPACE_SCOPES). Nobody has to reconnect for Sheets to work.

use talaria_body::percent_encode;
use talaria_gateway::provider::http;
use talaria_google_errors::GoogleError;

const SHEETS_ENDPOINT: &str = "https://sheets.googleapis.com/v4/spreadsheets";

/// A spreadsheet's identity plus one rectangle of its cells.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SheetContent {
    pub id: String,
    pub title: String,
    pub url: String,
    /// Every tab in the document, in document order — the names a caller can
    /// pass back as an A1 range prefix.
    pub tabs: Vec<String>,
    /// The A1 range these rows actually came from, as Google resolved it. Not
    /// the range asked for: an unqualified ask resolves to the first tab, and
    /// the caller should be told which one that was.
    pub range: String,
    /// Row-major cells, stringified. Google omits trailing empty cells, so
    /// rows are ragged; callers pad if they need a rectangle.
    pub rows: Vec<Vec<String>>,
}

/// What a write actually touched, straight from Google's own accounting.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SheetUpdate {
    pub id: String,
    pub url: String,
    pub range: String,
    pub updated_rows: i64,
    pub updated_columns: i64,
    pub updated_cells: i64,
}

fn sheet_url(id: &str) -> String {
    format!("https://docs.google.com/spreadsheets/d/{id}/edit")
}

fn str_at(v: &serde_json::Value, path: &[&str]) -> String {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return String::new(),
        }
    }
    cur.as_str().unwrap_or_default().to_string()
}

/// The document's identity and tab list, with no cell data. `includeGridData`
/// stays false on purpose: a large sheet's grid is megabytes, and the caller
/// asks for the cells it wants separately.
async fn sheet_meta(token: &str, id: &str) -> Result<serde_json::Value, GoogleError> {
    let res = http()
        .get(format!(
            "{SHEETS_ENDPOINT}/{}?includeGridData=false&fields=spreadsheetId,properties.title,sheets.properties.title",
            percent_encode(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets meta request: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "sheets meta failed: {status} {body}"
        )));
    }
    res.json()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets meta body: {e}")))
}

/// Read cells. `range` is A1 (`'Q3 forecast'!A1:D50`, or `Sheet1` for a whole
/// tab); None reads the first tab entire.
#[tracing::instrument(skip(token))]
pub async fn read_sheet_with_token(
    token: &str,
    id: &str,
    range: Option<&str>,
) -> Result<SheetContent, GoogleError> {
    let meta = sheet_meta(token, id).await?;
    let title = str_at(&meta, &["properties", "title"]);
    let tabs: Vec<String> = meta
        .get("sheets")
        .and_then(|s| s.as_array())
        .map(|rows| {
            rows.iter()
                .map(|s| str_at(s, &["properties", "title"]))
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // No range and no tabs means an empty document, not an error: answer with
    // the identity and no cells rather than inventing a range to ask for.
    let asked = match range {
        Some(r) if !r.trim().is_empty() => r.trim().to_string(),
        _ => match tabs.first() {
            Some(first) => first.clone(),
            None => {
                return Ok(SheetContent {
                    id: id.to_string(),
                    title,
                    url: sheet_url(id),
                    tabs,
                    range: String::new(),
                    rows: Vec::new(),
                });
            }
        },
    };
    let res = http()
        .get(format!(
            "{SHEETS_ENDPOINT}/{}/values/{}",
            percent_encode(id),
            percent_encode(&asked)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets values request: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "sheets read failed: {status} {body}"
        )));
    }
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets values body: {e}")))?;
    let rows = body
        .get("values")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .map(|cells| cells.iter().map(cell_text).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SheetContent {
        id: id.to_string(),
        title,
        url: sheet_url(id),
        tabs,
        // Google echoes the range it actually resolved; fall back to the ask.
        range: body
            .get("range")
            .and_then(|v| v.as_str())
            .unwrap_or(&asked)
            .to_string(),
        rows,
    })
}

/// A cell as the agent should see it. Numbers and booleans arrive untyped from
/// the values API, and `to_string()` on a JSON string would keep its quotes.
fn cell_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Write a rectangle. `USER_ENTERED` so a typed `=SUM(A1:A9)` becomes a
/// formula and `42%` becomes a percentage, the same as if a person had typed
/// it — `RAW` would store the literal text and surprise everyone.
#[tracing::instrument(skip(token, rows))]
pub async fn update_sheet_with_token(
    token: &str,
    id: &str,
    range: &str,
    rows: &[Vec<String>],
) -> Result<SheetUpdate, GoogleError> {
    let values: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::Value::Array(
                row.iter()
                    .map(|c| serde_json::Value::String(c.clone()))
                    .collect(),
            )
        })
        .collect();
    let res = http()
        .put(format!(
            "{SHEETS_ENDPOINT}/{}/values/{}?valueInputOption=USER_ENTERED",
            percent_encode(id),
            percent_encode(range)
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({ "range": range, "majorDimension": "ROWS", "values": values })
                .to_string(),
        )
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets update request: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "sheets update failed: {status} {body}"
        )));
    }
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("sheets update body: {e}")))?;
    let num = |key: &str| body.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
    Ok(SheetUpdate {
        id: id.to_string(),
        url: sheet_url(id),
        range: body
            .get("updatedRange")
            .and_then(|v| v.as_str())
            .unwrap_or(range)
            .to_string(),
        updated_rows: num("updatedRows"),
        updated_columns: num("updatedColumns"),
        updated_cells: num("updatedCells"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_keeps_its_text_and_unwraps_its_quotes() {
        assert_eq!(cell_text(&serde_json::json!("hello")), "hello");
        assert_eq!(cell_text(&serde_json::json!(42)), "42");
        assert_eq!(cell_text(&serde_json::json!(true)), "true");
        assert_eq!(cell_text(&serde_json::Value::Null), "");
    }

    #[test]
    fn a_missing_path_reads_as_empty_not_a_panic() {
        let v = serde_json::json!({ "properties": { "title": "Q3" } });
        assert_eq!(str_at(&v, &["properties", "title"]), "Q3");
        assert_eq!(str_at(&v, &["properties", "missing"]), "");
        assert_eq!(str_at(&v, &["nope", "title"]), "");
    }
}
