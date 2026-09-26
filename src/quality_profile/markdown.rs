//! The profile as Markdown, for the pages a person reads rather than the JSON an agent reads:
//! the body of a scan ticket and the "Quality profile" section of a decision-log file. Both come
//! from here, so a dimension reads the same wherever it is shown.
//!
//! A dimension is one bullet: its level and, beneath it, each reason with the ids of the nodes it
//! rests on; or, for a dimension that was not assessed, why. No number stands in for either, and
//! nothing here names a grade: the text is the JSON's reasons and ids, laid out for reading.
//!
//! # Placement
//! Layer 3, with the rest of the profile. The decision-log export (`queries::export_decision_log`)
//! is Layer 2 and must not import this module, so the CLI hands it [`decision_log_section`] to
//! call for each decision it writes. Pure text over a profile: the only graph read is the
//! decision's own [`quality_profile_of`].

use std::fmt::Write as _;

use crate::error::QueryError;
use crate::projector::GraphView;
use crate::Result;

use super::{quality_profile_of, Assessment, Dimension, Level, QualityProfile};

/// The name a dimension goes by in `docs/DECISION_SCORING.md`.
pub fn dimension_label(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Framing => "Framing",
        Dimension::Alternatives => "Alternatives",
        Dimension::Information => "Information",
        Dimension::Reasoning => "Reasoning",
        Dimension::ValuesTradeoffs => "Values / Tradeoffs",
        Dimension::BiasExposure => "Bias exposure",
        Dimension::Calibration => "Calibration",
    }
}

fn level_word(level: Level) -> &'static str {
    match level {
        Level::None => "none",
        Level::Partial => "partial",
        Level::Solid => "solid",
    }
}

/// Writes `text` on one line: a line break inside it cannot end the bullet it is written into.
fn push_one_line(out: &mut String, text: &str) {
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(word);
    }
}

/// Writes ` (`a`, `b`)` for the nodes a line rests on; nothing when it names none.
fn push_node_ids(out: &mut String, node_ids: &[String]) {
    for (index, id) in node_ids.iter().enumerate() {
        out.push_str(if index == 0 { " (`" } else { ", `" });
        out.push_str(id);
        out.push('`');
    }
    if !node_ids.is_empty() {
        out.push(')');
    }
}

/// One dimension as a Markdown bullet with no trailing newline: its level and one nested bullet
/// per reason with the ids behind it, or that it was not assessed and why.
pub fn dimension_markdown(dimension: Dimension, assessment: &Assessment) -> String {
    let label = dimension_label(dimension);
    match assessment {
        Assessment::Assessed { level, reasons, .. } => {
            let mut out = format!("- **{label}** — {}", level_word(*level));
            for reason in reasons {
                out.push_str("\n  - ");
                push_one_line(&mut out, &reason.text);
                push_node_ids(&mut out, &reason.node_ids);
            }
            out
        }
        Assessment::NotAssessed { why } => {
            let mut out = format!("- **{label}** — not assessed: ");
            push_one_line(&mut out, why);
            out
        }
    }
}

/// The whole profile as Markdown with no trailing newline: what the floors state, the seven
/// dimensions in order, and the lines that deserve a second look when there are any.
pub fn profile_markdown(profile: &QualityProfile) -> String {
    let mut out = format!(
        "Floor rules version {}: what the record states, not whether it is sound.\n",
        profile.floor_version
    );
    for (dimension, assessment) in profile.iter() {
        out.push('\n');
        out.push_str(&dimension_markdown(dimension, assessment));
    }
    if !profile.attention.is_empty() {
        out.push_str("\n\nWorth a second look:\n");
        for attention in &profile.attention {
            let _ = write!(out, "\n- **{}**: ", dimension_label(attention.dimension));
            push_one_line(&mut out, &attention.text);
            push_node_ids(&mut out, &attention.node_ids);
        }
    }
    out
}

/// The body of a decision's "Quality profile" section in the decision-log export. An error when
/// the decision is not in the graph: the export only asks for decisions it read a moment ago.
pub fn decision_log_section(graph: &impl GraphView, decision_id: &str) -> Result<String> {
    quality_profile_of(graph, decision_id)?
        .map(|profile| profile_markdown(&profile))
        .ok_or_else(|| {
            QueryError::Execution(format!(
                "decision {decision_id} has no profile: it is not in the graph"
            ))
            .into()
        })
}

#[cfg(test)]
pub(crate) mod tests;
