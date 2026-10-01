//! The one-line notice a read carries when the ledger holds annotation rows it could not read.
//!
//! An assessment (`decision.scored`) is Layer-3 enrichment appended to a shared, append-only
//! ledger, so a buggy or old producer, a partial import or a hand edit can leave a row nobody can
//! parse. Reads skip such a row (`events::validate_for_read`) instead of failing, and this module
//! is how they say so: the scan that finds the rows, the sentence that names them, and the two
//! places the sentence goes (a text line, or a `notice` key on a JSON object).

use serde_json::Value;

use crate::events::{validate_for_read, EventType, ReadEvent, TenantId, UnreadableAnnotation};
use crate::ledger::EventLedger;
use crate::Result;

/// How many ledger event ids the sentence lists before it says "and N more".
const LISTED_EVENT_IDS: usize = 5;

/// Every annotation row of `tenant_id`'s ledger that cannot be read, in ledger order. One pass
/// over the ledger; only annotation rows are parsed.
pub fn unreadable_annotations(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
) -> Result<Vec<UnreadableAnnotation>> {
    let mut rows = Vec::new();
    ledger.replay_from_for_tenant(tenant_id, 0, &mut |event| {
        if event.event_type.is_annotation() {
            if let Ok(ReadEvent::Unreadable(row)) = validate_for_read(event) {
                rows.push(row);
            }
        }
        Ok(())
    })?;
    Ok(rows)
}

/// The sentence for a non-empty list of unreadable rows: how many, which ledger events, and why
/// the first one failed. Empty input has no sentence; callers use [`notice_for`].
pub fn unreadable_annotations_notice(rows: &[UnreadableAnnotation]) -> String {
    let noun = if rows
        .iter()
        .all(|row| row.event_type == EventType::DecisionScored)
    {
        "assessment"
    } else {
        "annotation"
    };
    let ids = rows
        .iter()
        .take(LISTED_EVENT_IDS)
        .map(|row| {
            row.event_id
                .map_or_else(|| "?".to_owned(), |id| id.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let Some(first) = rows.first() else {
        return format!("no {noun} rows to report");
    };
    if rows.len() == 1 {
        return format!(
            "1 {noun} row could not be read and was skipped (ledger event {ids}: {}); answers leave it out",
            first.reason
        );
    }
    let more = rows.len().saturating_sub(LISTED_EVENT_IDS);
    let more = if more == 0 {
        String::new()
    } else {
        format!(" and {more} more")
    };
    format!(
        "{} {noun} rows could not be read and were skipped (ledger events {ids}{more}; first: {}); answers leave them out",
        rows.len(),
        first.reason
    )
}

/// The notice for `rows`, or `None` when every row could be read.
pub fn notice_for(rows: &[UnreadableAnnotation]) -> Option<String> {
    (!rows.is_empty()).then(|| unreadable_annotations_notice(rows))
}

/// Put `notice` on a JSON object answer as a top-level `notice` key. An answer that is not an
/// object (none of the read verbs return one) is left as it is.
pub fn annotate_json(answer: &mut Value, notice: &str) {
    if let Some(object) = answer.as_object_mut() {
        object.insert("notice".to_owned(), Value::String(notice.to_owned()));
    }
}

/// Put `notice` on rendered output: a top-level `notice` key when the output is a JSON object,
/// otherwise a final line `notice: …`, the way `--summary` output carries its other notices.
pub fn annotate_output(output: String, notice: &str) -> String {
    if let Ok(mut answer @ Value::Object(_)) = serde_json::from_str::<Value>(&output) {
        annotate_json(&mut answer, notice);
        let pretty = output.contains('\n');
        let rendered = if pretty {
            serde_json::to_string_pretty(&answer)
        } else {
            serde_json::to_string(&answer)
        };
        if let Ok(rendered) = rendered {
            return rendered;
        }
    }
    if output.is_empty() {
        format!("notice: {notice}")
    } else {
        format!("{output}\nnotice: {notice}")
    }
}

#[cfg(test)]
mod tests;
