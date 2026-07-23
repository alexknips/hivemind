# Misalignment Eval

Measures cross-track misalignment detection capability using a hand-authored gold corpus.

## What it measures

- **fire_accuracy** — fraction of cases where the detector's fire/dont-fire verdict matches gold.
- **counterparty_f1** — set-overlap F1 between predicted and gold counterparty actor sets.
- **per-dimension P/R** — for each derived-metadata dimension (premises, foreclosed options,
  disposition, goals, cross-track surfaces): what fraction of predicted items appear in gold
  (precision) and what fraction of gold items appear in predicted (recall).
- **macro_f1** — arithmetic mean of per-dimension F1 values.

## How to run

```bash
cargo run --bin misalignment-eval
cargo run --bin misalignment-eval -- --corpus path/to/other-corpus.yaml
```

Exits 0 always — this is a scorecard, not a pass/fail gate.

## Stub detectors

Two stub detectors are included to prove the eval discriminates:

| Detector     | Expected fire_accuracy | Expected macro_f1 |
|--------------|------------------------|-------------------|
| AlwaysFire   | 2/3 (≈ 0.667)          | 0.0               |
| AlwaysNoFire | 1/3 (≈ 0.333)          | 0.0               |

Both score well below 0.5, proving the eval is non-trivial. A perfect detector would score
1.0 / 1.0 across all dimensions.

## How to add a fixture

1. Open `benchmarks/misalignment/corpus.yaml`.
2. Append a new entry under `cases:` following the existing shape:

```yaml
  - id: CT-N-your-case-name      # stable, unique identifier
    name: "Brief human-readable name"
    archetype: contradicted_premise | foreclosed_option | non_fire

    tracks:
      track_a:
        actor: "team-a-id"
        captured_decisions:
          - decision_id: "d-unique-id"
            title: "..."
            rationale: >
              ...
            topic_keys: ["key1", "key2"]
      track_b:
        actor: "team-b-id"
        captured_decisions: [...]

    expected_derived_metadata:
      - decision_id: "d-unique-id"
        premises:
          - statement: "exact claim the detector should extract"
            provenance: stated | derived
            confidence_min: 0.6   # minimum acceptable confidence
        foreclosed_options: [...]
        disposition:
          value: willing | constrained | unwilling
          provenance: stated | derived
        goals: [...]
        cross_track_surfaces:
          - surface: "surface-label"
            kind: resource | interface | schema | constraint | concept

    expected_detector_output:
      fire: true | false
      alert_kind: contradicted_premise | foreclosed_option | goal_conflict | null
      counterparty: ["team-a-id"]   # actors to notify; empty if non-fire
      shared_surface: "surface-label" | null
      liveness: live | frozen
```

3. Run the eval to confirm the stub detectors still fail your new case.

## Corpus schema version

`fixture_schema_version: "1"` — bump if the fixture shape changes in a backwards-incompatible way.

## Archetype semantics

| Archetype              | Expected | Discriminator         |
|------------------------|----------|-----------------------|
| `contradicted_premise` | fire=true | A's derived premise is invalidated by B's decision |
| `foreclosed_option`    | fire=true | A's decision forecloses an option B's live need depends on |
| `non_fire`             | fire=false | No live counterparty can act (liveness=frozen) |

The liveness field is the key discriminator for non-fire precision cases.
