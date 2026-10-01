// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use crate::commands::Commands;
use crate::events::CaptureItem;
use crate::ledger::InMemoryEventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph};
use crate::Result;

use super::*;

fn decision(title: &str) -> CaptureItem {
    CaptureItem {
        kind: "decision".to_owned(),
        title: title.to_owned(),
        rationale: "Because the record says so".to_owned(),
        topic_keys: Vec::new(),
        evidence_ids: Vec::new(),
        options: None,
        chosen_option: None,
        extraction_confidence: 0.9,
        expressed_confidence: None,
        supersedes_id: None,
        restates_id: None,
        premised_on_ids: Vec::new(),
        supports_ids: Vec::new(),
        refutes_ids: Vec::new(),
        actor_id: None,
        accepted_by: Vec::new(),
        rejected_by: Vec::new(),
        blocked_actor_id: None,
        decision_id: None,
        participants: Vec::new(),
        session_initiator: None,
    }
}

/// Five decisions, `ids[0]` earliest, with a `SAME_AS` link for each `(later, earlier)` pair.
fn graph_with_links(links: &[(usize, usize)]) -> Result<(MemoryGraph, Vec<String>)> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    commands.record_evidence("human:alex", "so no capture is event 1")?;
    let recorded = commands.record_ingest_batch_classified(
        "agent:claude:classifier",
        &["batch-1".to_owned()],
        "claude-haiku-4-5-20251001",
        "2",
        ["alpha", "beta", "gamma", "delta", "epsilon"]
            .map(|name| decision(&format!("Decision {name}")))
            .into(),
        None,
    )?;
    let ids: Vec<String> = (0..5)
        .map(|index| format!("capture:{}:{index}", recorded.event_id))
        .collect();
    for &(later, earlier) in links {
        commands.link_same_as("human:alex", &ids[later], &ids[earlier])?;
    }
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok((graph, ids))
}

#[test]
fn links_group_decisions_in_either_direction_and_transitively() -> Result<()> {
    // 1 -> 0 and 2 -> 1 make {0, 1, 2}; 4 -> 3 makes {3, 4}. Written as (later, earlier), and
    // the group does not care which end an edge points from.
    let (graph, ids) = graph_with_links(&[(1, 0), (2, 1), (4, 3)])?;
    let mut groups = same_as_groups(&graph)?;
    groups.sort();

    let mut expected = vec![
        vec![ids[0].clone(), ids[1].clone(), ids[2].clone()],
        vec![ids[3].clone(), ids[4].clone()],
    ];
    expected.sort();
    assert_eq!(groups, expected);
    Ok(())
}

#[test]
fn no_link_means_no_group() -> Result<()> {
    let (graph, _) = graph_with_links(&[])?;
    assert!(same_as_groups(&graph)?.is_empty());
    assert!(SameAsLinks::load(&graph)?.is_empty());
    Ok(())
}

#[test]
fn fold_shows_the_earliest_matching_record_and_lists_every_other_record() -> Result<()> {
    // 0, 1 and 2 are one decision; 3 stands alone. Only 1, 2 and 3 matched: 0 is listed anyway.
    let (graph, ids) = graph_with_links(&[(1, 0), (2, 0)])?;
    let matched = vec![
        (ids[2].clone(), 3u32),
        (ids[3].clone(), 5),
        (ids[1].clone(), 7),
    ];

    let folded = fold_linked(
        &graph,
        matched,
        |(id, _)| id.as_str(),
        |shown, other| shown.1 += other.1,
    )?;

    assert_eq!(folded.len(), 2, "the three records fold into one entry");
    // The entry takes the place of the first record in the list, and shows the earliest one.
    assert_eq!(folded[0].item, (ids[1].clone(), 7 + 3));
    let listed: Vec<&str> = folded[0]
        .also_recorded_as
        .iter()
        .map(|copy| copy.decision_id.as_str())
        .collect();
    assert_eq!(listed, vec![ids[0].as_str(), ids[2].as_str()]);
    assert_eq!(folded[0].also_recorded_as[0].title, "Decision alpha");
    assert_eq!(folded[1].item, (ids[3].clone(), 5));
    assert!(folded[1].also_recorded_as.is_empty());
    Ok(())
}

#[test]
fn fold_passes_unlinked_results_through_in_order() -> Result<()> {
    let (graph, ids) = graph_with_links(&[(1, 0)])?;
    let results = vec![ids[4].clone(), ids[2].clone(), ids[3].clone()];

    let folded = fold_linked(&graph, results.clone(), |id| id.as_str(), |_, _| {})?;

    assert_eq!(
        folded
            .into_iter()
            .map(|entry| entry.item)
            .collect::<Vec<_>>(),
        results
    );
    Ok(())
}

#[test]
fn headings_list_titled_decisions_earliest_first() -> Result<()> {
    let (graph, ids) = graph_with_links(&[])?;
    let headings = decision_headings(&graph)?;

    assert_eq!(
        headings
            .iter()
            .map(|heading| heading.decision_id.as_str())
            .collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(headings[0].title, "Decision alpha");
    // One batch recorded them all, so they share a place in the ledger and are told apart by id.
    assert_eq!(headings[0].event_origin, headings[1].event_origin);
    Ok(())
}
