//! A/B uplift harness (Phase 1) — see bead hivemind-4ygu.
//!
//! Measures whether HiveMind's structured query output (treatment arm)
//! outperforms hand-authored ADR/MADR documents (control arm) on five
//! decision-recall task types. Both arms use the same model.
//!
//! Scoring:
//!   T1 (decision-recall):         token-F1 over decision title + options
//!   T2 (rationale-reconstruction): token-F1 over rationale + evidence text
//!   T3 (contradiction-detection):  binary accuracy (contested/disputed signal)
//!   T4 (onboarding):              token-F1 over decisions + evidence
//!   T5 (staleness-detection):     binary accuracy (refuted/superseded signal)
//!
//! Results are labeled PRELIMINARY (agent-only phase 1).
//!
//! Usage:
//!   cargo run --bin ab-eval                       # 5 tasks × 2 arms × 3 runs
//!   cargo run --bin ab-eval -- --dry-run          # PRELIMINARY template (no API)
//!   cargo run --bin ab-eval -- --task T3          # single task, 3 runs each arm
//!   cargo run --bin ab-eval -- --n-runs 5         # increase run count

use std::collections::HashSet;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-sonnet-4-6";
const MAX_TOKENS: u64 = 512;
const DEFAULT_RUNS: usize = 3;

// --------------------------------------------------------------------------
// Embedded contexts (compile-time)
// --------------------------------------------------------------------------

const T1_CONTROL: &str = include_str!("control/t1-cloud-region.md");
const T3_CONTROL: &str = include_str!("control/t3-hiring-contested.md");
const T4_CONTROL: &str = include_str!("control/t4-pricing-reversal.md");
const T5_CONTROL: &str = include_str!("control/t5-write-batching.md");

const T1_TREATMENT: &str = include_str!("treatment/t1-cloud-region.json");
const T3_TREATMENT: &str = include_str!("treatment/t3-hiring-contested.json");
const T4_TREATMENT: &str = include_str!("treatment/t4-pricing-reversal.json");
const T5_TREATMENT: &str = include_str!("treatment/t5-write-batching.json");

// --------------------------------------------------------------------------
// Score type
// --------------------------------------------------------------------------

enum ScoreType {
    TokenF1 { gold_text: &'static str },
    Binary { expect_positive: bool },
}

// --------------------------------------------------------------------------
// Task scenarios
// --------------------------------------------------------------------------

struct Scenario {
    id: &'static str,
    name: &'static str,
    question: &'static str,
    control_context: &'static str,
    treatment_context: &'static str,
    score_type: ScoreType,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            id: "T1",
            name: "decision-recall",
            question:
                "List all decisions described in this context. \
                       For each decision, state the options considered and which option was chosen.",
            control_context: T1_CONTROL,
            treatment_context: T1_TREATMENT,
            score_type: ScoreType::TokenF1 {
                // gold: decision title + all options (normalized union)
                gold_text: "host eu customer data eu central 1 eu central 1 eu west 1 us east 1",
            },
        },
        Scenario {
            id: "T2",
            name: "rationale-reconstruction",
            // T2 reuses the same case as T1 (cloud region), different question
            question: "Why was the cloud region decision made? \
                       What specific evidence supported the chosen option?",
            control_context: T1_CONTROL,
            treatment_context: T1_TREATMENT,
            score_type: ScoreType::TokenF1 {
                // gold: evidence texts (GDPR + RUM dashboard)
                gold_text: "gdpr data residency rum dashboard lower latency eu cohort central",
            },
        },
        Scenario {
            id: "T3",
            name: "contradiction-detection",
            question: "Are there any decisions in this context that are contested, \
                       disputed, or have conflicting positions from different stakeholders? \
                       Answer yes or no, and briefly explain.",
            control_context: T3_CONTROL,
            treatment_context: T3_TREATMENT,
            // gold: case has ProposedBy + RejectedBy on same node → contested = true
            score_type: ScoreType::Binary {
                expect_positive: true,
            },
        },
        Scenario {
            id: "T4",
            name: "onboarding",
            question: "I am new to this project. Summarize the pricing model history: \
                       what changed, why it was changed, and what the current status is.",
            control_context: T4_CONTROL,
            treatment_context: T4_TREATMENT,
            score_type: ScoreType::TokenF1 {
                // gold: both decisions + evidence + key rationale tokens
                gold_text:
                    "usage based pricing flat per seat supersedes bill shock churn q2 4 points",
            },
        },
        Scenario {
            id: "T5",
            name: "staleness-detection",
            question: "Is the write batching decision still sound? \
                       Have any of its underlying assumptions been proven wrong? \
                       Answer yes or no, and briefly explain.",
            control_context: T5_CONTROL,
            treatment_context: T5_TREATMENT,
            // gold: hypothesis refuted → answer should be "no longer sound / refuted"
            score_type: ScoreType::Binary {
                expect_positive: true,
            },
        },
    ]
}

// --------------------------------------------------------------------------
// Text normalization (mirrors fidelity-eval)
// --------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let stripped: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --------------------------------------------------------------------------
// Scoring
// --------------------------------------------------------------------------

fn score_token_f1(gold_text: &str, produced: &str) -> f64 {
    let gold: HashSet<String> = normalize(gold_text)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let prod: HashSet<String> = normalize(produced)
        .split_whitespace()
        .map(str::to_owned)
        .collect();

    let tp = gold.intersection(&prod).count();
    let fp = prod.difference(&gold).count();
    let fn_ = gold.difference(&prod).count();

    let p = if tp + fp == 0 {
        0.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let r = if tp + fn_ == 0 {
        0.0
    } else {
        tp as f64 / (tp + fn_) as f64
    };

    if p + r == 0.0 {
        0.0
    } else {
        2.0 * p * r / (p + r)
    }
}

fn score_binary(expect_positive: bool, produced: &str) -> f64 {
    let lower = produced.to_lowercase();
    // Positive signals: response affirms disputed/contested/refuted/superseded
    let positive_signals = [
        "yes",
        "contested",
        "disputed",
        "conflicting",
        "disagreement",
        "opposing",
        "unresolved",
        "refuted",
        "proven wrong",
        "no longer",
        "superseded",
        "outdated",
        "invalid",
        "incorrect assumption",
    ];
    let is_positive = positive_signals.iter().any(|s| lower.contains(s));
    if is_positive == expect_positive {
        1.0
    } else {
        0.0
    }
}

fn score(scenario: &Scenario, response: &str) -> f64 {
    match &scenario.score_type {
        ScoreType::TokenF1 { gold_text } => score_token_f1(gold_text, response),
        ScoreType::Binary { expect_positive } => score_binary(*expect_positive, response),
    }
}

// --------------------------------------------------------------------------
// Claude API call
// --------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(serde::Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct Usage {
    output_tokens: u64,
}

async fn call_claude(
    client: &reqwest::Client,
    api_key: &str,
    context: &str,
    question: &str,
) -> Result<(String, u64), Box<dyn std::error::Error + Send + Sync>> {
    let user_content =
        format!("Context:\n{context}\n\nQuestion: {question}\n\nAnswer concisely and directly.");
    let request = serde_json::json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "messages": [{"role": "user", "content": user_content}]
    });

    let response = client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("api returned {status}: {body}").into());
    }

    let resp: ApiResponse = response.json().await?;
    let text = resp
        .content
        .into_iter()
        .find(|b| b.block_type == "text")
        .and_then(|b| b.text)
        .ok_or("no text block in api response")?;

    Ok((text, resp.usage.output_tokens))
}

// --------------------------------------------------------------------------
// Per-arm result
// --------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ArmResult {
    score: f64,
    tokens: u64,
    runs: usize,
}

// --------------------------------------------------------------------------
// Main
// --------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let n_runs = parse_flag_usize(&args, "--n-runs", DEFAULT_RUNS);
    let task_filter: Option<String> = parse_flag_str(&args, "--task");

    let api_key = if dry_run {
        String::new()
    } else {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.trim().is_empty() => k,
            _ => {
                eprintln!("error: ANTHROPIC_API_KEY not set");
                eprintln!("hint: use --dry-run to generate a PRELIMINARY scorecard template");
                std::process::exit(1);
            }
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("reqwest client");

    let all_scenarios = scenarios();
    let run_scenarios: Vec<&Scenario> = all_scenarios
        .iter()
        .filter(|s| task_filter.as_deref().is_none_or(|t| s.id == t))
        .collect();

    println!("HiveMind A/B Uplift Scorecard — Phase 1");
    println!("PRELIMINARY — agent-only, 2026-07-13");
    if dry_run {
        println!("Mode: DRY-RUN (no API calls; scores are N/A)");
    } else {
        println!("Mode: LIVE (model: {MODEL}, {n_runs} run(s) per task per arm)");
    }
    println!("Corpus: benchmarks/fidelity/corpus.yaml (36 cases, 3 org bundles)");
    println!();
    println!(
        "{:<4} {:<30} {:<10} {:<10} {:<10} {:<6}",
        "Task", "Name", "Arm", "F1|Acc", "Tokens", "Runs"
    );
    println!("{}", "-".repeat(76));

    let mut control_f1s: Vec<f64> = Vec::new();
    let mut treatment_f1s: Vec<f64> = Vec::new();

    for scenario in &run_scenarios {
        let (ctl, trt) = if dry_run {
            let ctl = ArmResult {
                score: f64::NAN,
                tokens: 0,
                runs: 0,
            };
            let trt = ArmResult {
                score: f64::NAN,
                tokens: 0,
                runs: 0,
            };
            (ctl, trt)
        } else {
            let ctl = run_arm(&client, &api_key, scenario, false, n_runs).await;
            let trt = run_arm(&client, &api_key, scenario, true, n_runs).await;
            (ctl, trt)
        };

        let fmt_score = |s: f64| -> String {
            if s.is_nan() {
                "N/A".to_owned()
            } else {
                format!("{:.3}", s)
            }
        };
        let fmt_tokens = |t: u64, dry: bool| -> String {
            if dry {
                "N/A".to_owned()
            } else {
                t.to_string()
            }
        };
        let fmt_runs = |r: usize, dry: bool| -> String {
            if dry {
                "0".to_owned()
            } else {
                r.to_string()
            }
        };

        println!(
            "{:<4} {:<30} {:<10} {:<10} {:<10} {:<6}",
            scenario.id,
            scenario.name,
            "control",
            fmt_score(ctl.score),
            fmt_tokens(ctl.tokens, dry_run),
            fmt_runs(ctl.runs, dry_run),
        );
        println!(
            "{:<4} {:<30} {:<10} {:<10} {:<10} {:<6}",
            "",
            "",
            "treatment",
            fmt_score(trt.score),
            fmt_tokens(trt.tokens, dry_run),
            fmt_runs(trt.runs, dry_run),
        );

        let uplift_str = if dry_run {
            "N/A".to_owned()
        } else {
            let d = trt.score - ctl.score;
            if d >= 0.0 {
                format!("+{:.3}", d)
            } else {
                format!("{:.3}", d)
            }
        };
        println!("{:<4} {:<30} {:<10} {:<10}", "", "", "Δ uplift", uplift_str);
        println!();

        if !dry_run && !ctl.score.is_nan() {
            control_f1s.push(ctl.score);
        }
        if !dry_run && !trt.score.is_nan() {
            treatment_f1s.push(trt.score);
        }
    }

    println!("{}", "=".repeat(76));
    println!("Aggregate");
    if dry_run {
        println!("  Macro-F1 control:   N/A");
        println!("  Macro-F1 treatment: N/A");
        println!("  Net uplift:         N/A");
    } else {
        let macro_ctl = if control_f1s.is_empty() {
            0.0
        } else {
            control_f1s.iter().sum::<f64>() / control_f1s.len() as f64
        };
        let macro_trt = if treatment_f1s.is_empty() {
            0.0
        } else {
            treatment_f1s.iter().sum::<f64>() / treatment_f1s.len() as f64
        };
        let net = macro_trt - macro_ctl;
        println!("  Macro-F1 control:   {:.3}", macro_ctl);
        println!("  Macro-F1 treatment: {:.3}", macro_trt);
        println!(
            "  Net uplift:         {}",
            if net >= 0.0 {
                format!("+{:.3}", net)
            } else {
                format!("{:.3}", net)
            }
        );
    }
    println!();
    println!("Note: Phase 1 scores are agent-only (synthetic corpus). Phase 2");
    println!("will replace control arm with real organization ADR archives.");
}

async fn run_arm(
    client: &reqwest::Client,
    api_key: &str,
    scenario: &Scenario,
    treatment: bool,
    n_runs: usize,
) -> ArmResult {
    let context = if treatment {
        scenario.treatment_context
    } else {
        scenario.control_context
    };
    let arm_label = if treatment { "treatment" } else { "control" };

    let mut scores: Vec<f64> = Vec::new();
    let mut total_tokens: u64 = 0;

    for run in 0..n_runs {
        match call_claude(client, api_key, context, scenario.question).await {
            Ok((text, tokens)) => {
                let s = score(scenario, &text);
                eprintln!(
                    "  {} {} run {}/{}: score={:.3} tokens={}",
                    scenario.id,
                    arm_label,
                    run + 1,
                    n_runs,
                    s,
                    tokens
                );
                scores.push(s);
                total_tokens += tokens;
            }
            Err(e) => {
                eprintln!(
                    "  {} {} run {}/{}: error: {e}",
                    scenario.id,
                    arm_label,
                    run + 1,
                    n_runs
                );
                scores.push(0.0);
            }
        }
    }

    let mean_score = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };
    let mean_tokens = if scores.is_empty() {
        0
    } else {
        total_tokens / scores.len() as u64
    };

    ArmResult {
        score: mean_score,
        tokens: mean_tokens,
        runs: scores.len(),
    }
}

// --------------------------------------------------------------------------
// Arg parsing helpers
// --------------------------------------------------------------------------

fn parse_flag_usize(args: &[String], flag: &str, default: usize) -> usize {
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        if a == flag {
            if let Some(v) = iter.next() {
                return v.parse().unwrap_or(default);
            }
        }
    }
    default
}

fn parse_flag_str(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter().peekable();
    while let Some(a) = iter.next() {
        if a == flag {
            return iter.next().cloned();
        }
    }
    None
}
