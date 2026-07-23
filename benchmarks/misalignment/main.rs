//! Cross-track misalignment evaluator.
//!
//! Reads benchmarks/misalignment/corpus.yaml, runs registered detectors, and
//! scores each against the gold corpus on:
//!
//! - fire_accuracy: fraction of cases with correct fire/dont-fire verdict
//! - counterparty_f1: set-overlap F1 for predicted vs gold counterparties
//! - per-dimension P/R: for premises, foreclosed_options, disposition, goals,
//!   cross_track_surfaces — normalized substring match against gold strings
//! - macro_f1: arithmetic mean of per-dimension F1 values
//!
//! Stub detectors (AlwaysFire, AlwaysNoFire) are included to prove the eval
//! discriminates. Both should score well below 0.5 macro-F1.
//!
//! Exits 0 always — this is a scorecard, not a gate.
//!
//! Usage:
//!   cargo run --bin misalignment-eval
//!   cargo run --bin misalignment-eval -- --corpus path/to/corpus.yaml

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use serde::Deserialize;

// --------------------------------------------------------------------------
// Corpus types (fixture format)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub archetype: String,
    pub tracks: Tracks,
    pub expected_derived_metadata: Vec<GoldDecisionMetadata>,
    pub expected_detector_output: GoldDetectorOutput,
}

#[derive(Debug, Deserialize)]
pub struct Tracks {
    pub track_a: Track,
    pub track_b: Track,
}

#[derive(Debug, Deserialize)]
pub struct Track {
    pub actor: String,
    pub captured_decisions: Vec<CapturedDecision>,
}

#[derive(Debug, Deserialize)]
pub struct CapturedDecision {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldDecisionMetadata {
    pub decision_id: String,
    #[serde(default)]
    pub premises: Vec<GoldStatement>,
    #[serde(default)]
    pub foreclosed_options: Vec<GoldStatement>,
    #[serde(default)]
    pub disposition: Option<GoldDisposition>,
    #[serde(default)]
    pub goals: Vec<GoldStatement>,
    #[serde(default)]
    pub cross_track_surfaces: Vec<GoldSurface>,
}

#[derive(Debug, Deserialize)]
pub struct GoldStatement {
    pub statement: String,
    pub provenance: String,
    #[allow(dead_code)]
    pub confidence_min: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GoldDisposition {
    pub value: String,
    pub provenance: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldSurface {
    pub surface: String,
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldDetectorOutput {
    pub fire: bool,
    pub alert_kind: Option<String>,
    pub counterparty: Vec<String>,
    pub shared_surface: Option<String>,
    pub liveness: String,
}

// --------------------------------------------------------------------------
// Detector interface
// --------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DetectorOutput {
    pub fire: bool,
    pub alert_kind: Option<String>,
    pub counterparty: Vec<String>,
    pub shared_surface: Option<String>,
    pub liveness: Option<String>,
    pub derived_metadata: Vec<DerivedMetadataOutput>,
}

#[derive(Debug, Clone, Default)]
pub struct DerivedMetadataOutput {
    pub decision_id: String,
    pub premises: Vec<String>,
    pub foreclosed_options: Vec<String>,
    pub disposition: Option<String>,
    pub goals: Vec<String>,
    pub surfaces: Vec<String>,
}

pub trait MisalignmentDetector {
    fn name(&self) -> &str;
    fn detect(&self, fixture: &Fixture) -> DetectorOutput;
}

// --------------------------------------------------------------------------
// Stub detectors
// --------------------------------------------------------------------------

pub struct AlwaysFire;

impl MisalignmentDetector for AlwaysFire {
    fn name(&self) -> &str {
        "AlwaysFire"
    }

    fn detect(&self, fixture: &Fixture) -> DetectorOutput {
        // Collect all actors as counterparty (a maximally wrong guess that's
        // still non-empty, so counterparty scoring exercises recall).
        let counterparty = vec![
            fixture.tracks.track_a.actor.clone(),
            fixture.tracks.track_b.actor.clone(),
        ];
        DetectorOutput {
            fire: true,
            alert_kind: Some("unknown".to_owned()),
            counterparty,
            shared_surface: None,
            liveness: Some("live".to_owned()),
            derived_metadata: vec![],
        }
    }
}

pub struct AlwaysNoFire;

impl MisalignmentDetector for AlwaysNoFire {
    fn name(&self) -> &str {
        "AlwaysNoFire"
    }

    fn detect(&self, _fixture: &Fixture) -> DetectorOutput {
        DetectorOutput {
            fire: false,
            alert_kind: None,
            counterparty: vec![],
            shared_surface: None,
            liveness: Some("frozen".to_owned()),
            derived_metadata: vec![],
        }
    }
}

// --------------------------------------------------------------------------
// Scoring
// --------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn substring_match(predicted: &str, gold: &str) -> bool {
    let p = normalize(predicted);
    let g = normalize(gold);
    p.contains(&g) || g.contains(&p)
}

fn set_f1(predicted: &[String], gold: &[String]) -> (f64, f64, f64) {
    if gold.is_empty() && predicted.is_empty() {
        return (1.0, 1.0, 1.0);
    }
    if gold.is_empty() {
        return (0.0, 1.0, 0.0);
    }
    if predicted.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let pred_set: BTreeSet<String> = predicted.iter().map(|s| normalize(s)).collect();
    let gold_set: BTreeSet<String> = gold.iter().map(|s| normalize(s)).collect();
    let intersection = pred_set.intersection(&gold_set).count() as f64;
    let precision = intersection / pred_set.len() as f64;
    let recall = intersection / gold_set.len() as f64;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    (precision, recall, f1)
}

fn string_list_pr(predicted: &[String], gold: &[String]) -> (f64, f64) {
    if gold.is_empty() {
        return if predicted.is_empty() {
            (1.0, 1.0)
        } else {
            (0.0, 1.0)
        };
    }
    if predicted.is_empty() {
        return (0.0, 0.0);
    }
    let tp_recall = gold
        .iter()
        .filter(|g| predicted.iter().any(|p| substring_match(p, g)))
        .count() as f64;
    let tp_precision = predicted
        .iter()
        .filter(|p| gold.iter().any(|g| substring_match(p, g)))
        .count() as f64;
    let precision = tp_precision / predicted.len() as f64;
    let recall = tp_recall / gold.len() as f64;
    (precision, recall)
}

#[derive(Debug, Default)]
struct DimScores {
    premises_p: f64,
    premises_r: f64,
    foreclosed_p: f64,
    foreclosed_r: f64,
    disposition_p: f64,
    disposition_r: f64,
    goals_p: f64,
    goals_r: f64,
    surfaces_p: f64,
    surfaces_r: f64,
}

impl DimScores {
    fn macro_f1(&self) -> f64 {
        let dims = [
            (self.premises_p, self.premises_r),
            (self.foreclosed_p, self.foreclosed_r),
            (self.disposition_p, self.disposition_r),
            (self.goals_p, self.goals_r),
            (self.surfaces_p, self.surfaces_r),
        ];
        let f1s: Vec<f64> = dims
            .iter()
            .map(|(p, r)| {
                if p + r > 0.0 {
                    2.0 * p * r / (p + r)
                } else {
                    0.0
                }
            })
            .collect();
        f1s.iter().sum::<f64>() / f1s.len() as f64
    }
}

fn score_metadata(
    predicted_map: &HashMap<String, DerivedMetadataOutput>,
    gold_meta: &[GoldDecisionMetadata],
) -> DimScores {
    let mut all_premises_p = vec![];
    let mut all_premises_r = vec![];
    let mut all_foreclosed_p = vec![];
    let mut all_foreclosed_r = vec![];
    let mut all_disposition_p = vec![];
    let mut all_disposition_r = vec![];
    let mut all_goals_p = vec![];
    let mut all_goals_r = vec![];
    let mut all_surfaces_p = vec![];
    let mut all_surfaces_r = vec![];

    for gm in gold_meta {
        let pred = predicted_map
            .get(&gm.decision_id)
            .cloned()
            .unwrap_or_default();

        let gold_premises: Vec<String> = gm.premises.iter().map(|p| p.statement.clone()).collect();
        let (p, r) = string_list_pr(&pred.premises, &gold_premises);
        all_premises_p.push(p);
        all_premises_r.push(r);

        let gold_foreclosed: Vec<String> = gm
            .foreclosed_options
            .iter()
            .map(|f| f.statement.clone())
            .collect();
        let (p, r) = string_list_pr(&pred.foreclosed_options, &gold_foreclosed);
        all_foreclosed_p.push(p);
        all_foreclosed_r.push(r);

        let gold_disposition: Vec<String> = gm
            .disposition
            .as_ref()
            .map(|d| vec![d.value.clone()])
            .unwrap_or_default();
        let pred_disposition: Vec<String> = pred.disposition.into_iter().collect();
        let (p, r) = string_list_pr(&pred_disposition, &gold_disposition);
        all_disposition_p.push(p);
        all_disposition_r.push(r);

        let gold_goals: Vec<String> = gm.goals.iter().map(|g| g.statement.clone()).collect();
        let (p, r) = string_list_pr(&pred.goals, &gold_goals);
        all_goals_p.push(p);
        all_goals_r.push(r);

        let gold_surfaces: Vec<String> = gm
            .cross_track_surfaces
            .iter()
            .map(|s| s.surface.clone())
            .collect();
        let (p, r) = string_list_pr(&pred.surfaces, &gold_surfaces);
        all_surfaces_p.push(p);
        all_surfaces_r.push(r);
    }

    let avg = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };

    DimScores {
        premises_p: avg(&all_premises_p),
        premises_r: avg(&all_premises_r),
        foreclosed_p: avg(&all_foreclosed_p),
        foreclosed_r: avg(&all_foreclosed_r),
        disposition_p: avg(&all_disposition_p),
        disposition_r: avg(&all_disposition_r),
        goals_p: avg(&all_goals_p),
        goals_r: avg(&all_goals_r),
        surfaces_p: avg(&all_surfaces_p),
        surfaces_r: avg(&all_surfaces_r),
    }
}

struct CaseResult {
    fire_correct: bool,
    counterparty_f1: f64,
    dim: DimScores,
}

fn score_case(fixture: &Fixture, output: &DetectorOutput) -> CaseResult {
    let gold = &fixture.expected_detector_output;

    let fire_correct = output.fire == gold.fire;

    let counterparty_f1 = if gold.fire {
        let (_, _, f1) = set_f1(&output.counterparty, &gold.counterparty);
        f1
    } else {
        // non-fire cases: empty counterparty is expected
        if output.counterparty.is_empty() {
            1.0
        } else {
            0.0
        }
    };

    let pred_map: HashMap<String, DerivedMetadataOutput> = output
        .derived_metadata
        .iter()
        .map(|m| (m.decision_id.clone(), m.clone()))
        .collect();

    let dim = score_metadata(&pred_map, &fixture.expected_derived_metadata);

    CaseResult {
        fire_correct,
        counterparty_f1,
        dim,
    }
}

// --------------------------------------------------------------------------
// Report
// --------------------------------------------------------------------------

fn run_detector(detector: &dyn MisalignmentDetector, corpus: &[Fixture]) {
    let mut results: Vec<CaseResult> = vec![];
    for fixture in corpus {
        let output = detector.detect(fixture);
        results.push(score_case(fixture, &output));
    }

    let n = results.len() as f64;
    let fire_accuracy = results.iter().filter(|r| r.fire_correct).count() as f64 / n;
    let cp_f1 = results.iter().map(|r| r.counterparty_f1).sum::<f64>() / n;

    let avg_dim = |f: fn(&DimScores) -> f64| results.iter().map(|r| f(&r.dim)).sum::<f64>() / n;

    let prem_p = avg_dim(|d| d.premises_p);
    let prem_r = avg_dim(|d| d.premises_r);
    let fore_p = avg_dim(|d| d.foreclosed_p);
    let fore_r = avg_dim(|d| d.foreclosed_r);
    let disp_p = avg_dim(|d| d.disposition_p);
    let disp_r = avg_dim(|d| d.disposition_r);
    let goal_p = avg_dim(|d| d.goals_p);
    let goal_r = avg_dim(|d| d.goals_r);
    let surf_p = avg_dim(|d| d.surfaces_p);
    let surf_r = avg_dim(|d| d.surfaces_r);

    let macro_f1 = results.iter().map(|r| r.dim.macro_f1()).sum::<f64>() / n;

    println!("=== {} ===", detector.name());
    println!("  fire_accuracy         : {:.3}", fire_accuracy);
    println!("  counterparty_f1       : {:.3}", cp_f1);
    println!("  premises     P={:.3} R={:.3}", prem_p, prem_r);
    println!("  foreclosed   P={:.3} R={:.3}", fore_p, fore_r);
    println!("  disposition  P={:.3} R={:.3}", disp_p, disp_r);
    println!("  goals        P={:.3} R={:.3}", goal_p, goal_r);
    println!("  surfaces     P={:.3} R={:.3}", surf_p, surf_r);
    println!("  macro_f1              : {:.3}", macro_f1);

    let per_case_label = if fire_accuracy < 1.0 || macro_f1 < 0.5 {
        "FAILING (expected — stub detectors prove eval discriminates)"
    } else {
        "PASSING"
    };
    println!("  verdict               : {}", per_case_label);
    println!();
}

// --------------------------------------------------------------------------
// Main
// --------------------------------------------------------------------------

fn main() {
    let mut corpus_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benchmarks/misalignment/corpus.yaml");

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--corpus" && i + 1 < args.len() {
            corpus_path = PathBuf::from(&args[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }

    let content = std::fs::read_to_string(&corpus_path)
        .unwrap_or_else(|e| panic!("cannot read corpus at {}: {e}", corpus_path.display()));

    let corpus: Corpus =
        serde_yaml::from_str(&content).unwrap_or_else(|e| panic!("cannot parse corpus YAML: {e}"));

    println!("misalignment-eval  corpus={}", corpus_path.display());
    println!("cases: {}", corpus.cases.len());
    println!();

    let detectors: Vec<Box<dyn MisalignmentDetector>> =
        vec![Box::new(AlwaysFire), Box::new(AlwaysNoFire)];

    for detector in &detectors {
        run_detector(detector.as_ref(), &corpus.cases);
    }

    println!("Both stubs score <1.0 fire_accuracy and are marked FAILING.");
    println!("The eval corpus discriminates fire/dont-fire: a perfect detector scores 1.0 on all metrics.");
}
