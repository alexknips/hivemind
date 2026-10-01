#!/usr/bin/env python3
"""Score the shadow-run gate answers against Worker A's captures (hivemind-gwdt).

  analyze.py CALLS.jsonl RESULTS.jsonl EVENTS.jsonl > summary.json

Unit = one Worker A call that had signal text. Gate "positive" = send to Worker A.
Labels come from Worker A's actual captures: `any` (>=1 capture of any kind) and
`decision` (>=1 capture of kind "decision"). Calls split into a fit half and a
held-out half by a hash of their session id, so no session spans both halves.
Prints aggregate numbers only; no transcript text.
"""
import hashlib
import json
import math
import sys
from collections import Counter, defaultdict

TIERS = {"crew": 0, "mayor": 0, "oracle": 0, "deacon": 1, "polecat": 1, "refinery": 1,
         "hivemind-company-analyst": 1, "hivemind-crew": 0, "witness": 2, "dog": 2, "boot": 2}
SPLIT_KEYS = ["chose", "reason", "human_ruling", "open_choice", "observation", "handoff", "blocked", "routine"]
RECALL_TARGETS = (0.90, 0.95, 0.98)


def half_of(row):
    key = (row["sessions"] or [str(row["event_id"])])[0] or str(row["event_id"])
    return int(hashlib.md5(key.encode()).hexdigest(), 16) % 2  # 0 = fit, 1 = held out


def solve(a, b):
    n = len(b)
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(m[r][c]))
        m[c], m[p] = m[p], m[c]
        for r in range(n):
            if r != c:
                f = m[r][c] / m[c][c]
                for k in range(c, n + 1):
                    m[r][k] -= f * m[c][k]
    return [m[i][n] / m[i][i] for i in range(n)]


def fit_logistic(x, y, l2=1.0, iters=50):
    """IRLS with a ridge penalty (not on the intercept). x rows exclude the intercept."""
    d = len(x[0]) + 1
    xs = [[1.0] + r for r in x]
    w = [0.0] * d
    for _ in range(iters):
        g = [0.0] * d
        h = [[0.0] * d for _ in range(d)]
        for r, yi in zip(xs, y):
            z = sum(wi * ri for wi, ri in zip(w, r))
            p = 1 / (1 + math.exp(-max(-30, min(30, z))))
            for i in range(d):
                g[i] += (p - yi) * r[i]
                for j in range(d):
                    h[i][j] += p * (1 - p) * r[i] * r[j]
        for i in range(1, d):
            g[i] += l2 * w[i]
            h[i][i] += l2
        h[0][0] += 1e-9
        step = solve(h, g)
        w = [wi - si for wi, si in zip(w, step)]
        if max(abs(s) for s in step) < 1e-8:
            break
    return w


def predict(w, x):
    return [1 / (1 + math.exp(-max(-30, min(30, w[0] + sum(wi * xi for wi, xi in zip(w[1:], r)))))) for r in x]


def standardize(fit_x, all_x):
    cols = list(zip(*fit_x))
    mu = [sum(c) / len(c) for c in cols]
    sd = [(sum((v - m) ** 2 for v in c) / len(c)) ** 0.5 or 1.0 for c, m in zip(cols, mu)]
    return [[(v - m) / s for v, m, s in zip(r, mu, sd)] for r in all_x]


def auc(scores, labels):
    pos = [s for s, y in zip(scores, labels) if y]
    neg = [s for s, y in zip(scores, labels) if not y]
    if not pos or not neg:
        return None
    wins = sum((p > n) + 0.5 * (p == n) for p in pos for n in neg)
    return wins / (len(pos) * len(neg))


def wilson(k, n, z=1.96):
    if not n:
        return None
    p = k / n
    d = 1 + z * z / n
    c = p + z * z / (2 * n)
    m = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return [round((c - m) / d, 3), round((c + m) / d, 3)]


def confusion(scores, rows, label, thr):
    tp = fp = fn = tn = 0
    lost = dec_lost = 0
    tok_skipped = sum(r["tok"] for s, r in zip(scores, rows) if s < thr)
    tok_all = sum(r["tok"] for r in rows)
    for s, r in zip(scores, rows):
        y = label(r)
        if s >= thr:
            tp += y
            fp += not y
        else:
            fn += y
            tn += not y
            lost += r["n_captures"]
            dec_lost += sum(1 for k in r["kinds"] if k == "decision")
    n = tp + fp + fn + tn
    return {
        "n": n, "tp": tp, "fp": fp, "fn": fn, "tn": tn,
        "precision": tp / (tp + fp) if tp + fp else None,
        "recall": tp / (tp + fn) if tp + fn else None,
        "empties_skipped": tn / (tn + fp) if tn + fp else None,
        "calls_skipped": (tn + fn) / n if n else None,
        "tokens_skipped": tok_skipped / tok_all if tok_all else None,
        "captures_lost": lost, "decision_captures_lost": dec_lost,
        "recall_ci95": wilson(tp, tp + fn), "empties_skipped_ci95": wilson(tn, tn + fp),
    }


def pick_threshold(scores, rows, label, target):
    """Highest threshold whose recall on these rows is >= target."""
    pos = sorted((s for s, r in zip(scores, rows) if label(r)), reverse=True)
    if not pos:
        return 0.0
    need = math.ceil(target * len(pos))
    return pos[need - 1]


def main(calls_path, results_path, events_path):
    calls = [json.loads(l) for l in open(calls_path, encoding="utf-8")]
    calls = [c for c in calls if c["chars"] > 0]
    res = {}
    for l in open(results_path, encoding="utf-8"):
        d = json.loads(l)
        if not d.get("error"):
            res[(d["event_id"], d["pass"])] = d
    errors = Counter()
    for l in open(results_path, encoding="utf-8"):
        d = json.loads(l)
        if d.get("error"):
            errors[d["pass"]] += 1
    cap_chars = {}
    for l in open(events_path, encoding="utf-8"):
        e = json.loads(l)
        if e["event_type"] == "ingest.batch_classified":
            p = e["payload"] if isinstance(e["payload"], dict) else json.loads(e["payload"])
            cap_chars[e["event_id"]] = len(json.dumps(p.get("captures") or []))

    rows = [c for c in calls if (c["event_id"], "q1") in res and (c["event_id"], "split") in res]
    out = {"calls_with_text": len(calls), "scored": len(rows), "errors": dict(errors)}
    for r in rows:
        r["half"] = half_of(r)
        d = res[(r["event_id"], "q1")]  # the q1 call read the same text Worker A did: input-token proxy
        r["tok"] = sum(d[k] or 0 for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
    fit = [r for r in rows if r["half"] == 0]
    held = [r for r in rows if r["half"] == 1]
    out["fit_n"], out["held_n"] = len(fit), len(held)
    labels = {"any": lambda r: int(r["n_captures"] > 0), "decision": lambda r: int("decision" in r["kinds"])}
    out["label_counts"] = {
        name: {"all": sum(f(r) for r in rows), "fit": sum(f(r) for r in fit), "held": sum(f(r) for r in held)}
        for name, f in labels.items()
    }
    out["real_empty_calls"] = sum(1 for r in rows if r["n_captures"] == 0)

    q1 = lambda r: res[(r["event_id"], "q1")]["reply"]["p_yes"]
    q1_yes = lambda r: 1.0 if res[(r["event_id"], "q1")]["reply"]["answer"] == "yes" else 0.0

    def free_feats(r):
        t = TIERS.get(r["role"], 1)
        user_lines = sum(1 for ln in r["text"].splitlines() if ln.startswith("[user]"))
        return [math.log1p(r["chars"]), math.log1p(user_lines), math.log1p(r["n_batches"]),
                float(t == 0), float(t == 2)]

    def split_feats(r):
        rep = res[(r["event_id"], "split")]["reply"]
        return [rep[k] for k in SPLIT_KEYS]

    arms = {}
    # Arms that need no fitting: q1 probability, q1 plain answer.
    arms["q1_p_yes"] = {"score": {r["event_id"]: q1(r) for r in rows}}
    arms["q1_answer"] = {"score": {r["event_id"]: q1_yes(r) for r in rows}}
    # Fitted arms: one logistic fit per label on the fit half, scored on every row.
    for name, feat in (("split_fitted", split_feats), ("free_role_size", free_feats),
                       ("split_plus_free", lambda r: split_feats(r) + free_feats(r))):
        arms[name] = {"score": {}, "weights": {}}
        for lab, f in labels.items():
            fx = [feat(r) for r in fit]
            all_x = standardize(fx, [feat(r) for r in rows])
            fit_x = standardize(fx, fx)
            w = fit_logistic(fit_x, [f(r) for r in fit])
            sc = predict(w, all_x)
            arms[name]["score"][lab] = {r["event_id"]: s for r, s in zip(rows, sc)}
            arms[name]["weights"][lab] = [round(v, 3) for v in w]

    def scores_for(arm, lab, subset):
        s = arms[arm]["score"]
        if arm in ("q1_p_yes", "q1_answer"):
            return [s[r["event_id"]] for r in subset]
        return [s[lab][r["event_id"]] for r in subset]

    out["arms"] = {}
    for arm in arms:
        out["arms"][arm] = {}
        for lab, f in labels.items():
            entry = {}
            fs, hs, al = (scores_for(arm, lab, fit), scores_for(arm, lab, held), scores_for(arm, lab, rows))
            entry["auc_all"] = auc(al, [f(r) for r in rows])
            entry["auc_held"] = auc(hs, [f(r) for r in held])
            entry["auc_fit"] = auc(fs, [f(r) for r in fit])
            if arm == "q1_answer":
                entry["at_yes"] = {"fit": confusion(fs, fit, f, 1.0), "held": confusion(hs, held, f, 1.0),
                                   "all": confusion(al, rows, f, 1.0)}
            else:
                entry["at_half"] = {"held": confusion(hs, held, f, 0.5), "all": confusion(al, rows, f, 0.5)}
                entry["targets"] = {}
                for tgt in RECALL_TARGETS:
                    thr = pick_threshold(fs, fit, f, tgt)
                    entry["targets"][str(tgt)] = {"threshold": thr, "fit": confusion(fs, fit, f, thr),
                                                  "held": confusion(hs, held, f, thr)}
            out["arms"][arm][lab] = entry
    out["weights"] = {a: arms[a].get("weights") for a in arms if "weights" in arms[a]}

    # Where the gate disagrees with Worker A, by role tier and by kind (aggregates only).
    thr95 = out["arms"]["q1_p_yes"]["any"]["targets"]["0.95"]["threshold"]
    by_role = defaultdict(lambda: Counter())
    for r in rows:
        k = r["role"] or "?"
        by_role[k]["calls"] += 1
        by_role[k]["positive"] += int(r["n_captures"] > 0)
        by_role[k]["q1_skipped_at_fit95"] += int(q1(r) < thr95)
    out["by_role"] = {k: dict(v) for k, v in sorted(by_role.items())}
    kind_total, kind_skipped = Counter(), Counter()
    for r in rows:
        for k in r["kinds"]:
            kind_total[k] += 1
            kind_skipped[k] += int(q1(r) < thr95)
    out["q1_fit95_captures_in_skipped_calls_by_kind"] = {k: [kind_skipped[k], kind_total[k]] for k in kind_total}

    # Zero-model rules: skip every call from the named roles, no fitting, whole week.
    out["role_rules"] = {}
    all_captures = sum(r["n_captures"] for r in rows)
    for name, roles in (("skip_boot", {"boot"}), ("skip_boot_witness", {"boot", "witness"}),
                        ("skip_boot_witness_dog", {"boot", "witness", "dog"})):
        sk = [r for r in rows if r["role"] in roles]
        pos = [r for r in sk if r["n_captures"]]
        out["role_rules"][name] = {
            "calls_skipped": len(sk), "calls_skipped_frac": round(len(sk) / len(rows), 3),
            "tokens_skipped_frac": round(sum(r["tok"] for r in sk) / sum(r["tok"] for r in rows), 3),
            "empties_skipped": len(sk) - len(pos), "empties_skipped_frac": round((len(sk) - len(pos)) / out["real_empty_calls"], 3),
            "positive_calls_lost": len(pos), "recall": round(1 - len(pos) / out["label_counts"]["any"]["all"], 3),
            "recall_ci95": wilson(out["label_counts"]["any"]["all"] - len(pos), out["label_counts"]["any"]["all"]),
            "captures_lost": sum(r["n_captures"] for r in pos), "captures_total": all_captures,
            "decision_captures_lost": sum(1 for r in pos for k in r["kinds"] if k == "decision"),
            "kinds_lost": dict(Counter(k for r in pos for k in r["kinds"])),
        }

    # Cost of the gate runs, and what the same text cost Worker A (proxy).
    out["cost"] = {}
    for which in ("q1", "split"):
        rs = [res[(r["event_id"], which)] for r in rows]
        tin = sum((d["input_tokens"] or 0) + (d["cache_creation_input_tokens"] or 0) + (d["cache_read_input_tokens"] or 0) for d in rs)
        tout = sum(d["output_tokens"] or 0 for d in rs)
        out["cost"][which] = {
            "calls": len(rs), "input_tokens": tin, "output_tokens": tout,
            "notional_usd": round(sum(d["cost_usd_notional"] or 0 for d in rs), 4),
            "mean_wall_s": round(sum(d["wall_s"] for d in rs) / len(rs), 2),
            "mean_api_ms": round(sum(d["duration_ms"] or 0 for d in rs) / len(rs)),
        }
    batches = sum(r["n_batches"] for r in rows)
    out["batches_covered"] = batches
    days = Counter(r["ts"][:10] for r in rows)
    out["real_calls_by_day"] = dict(sorted(days.items()))
    out["worker_a_output_tokens_est"] = round(sum(cap_chars.get(r["event_id"], 0) for r in rows) / 3.5)
    out["worker_a_input_tokens_proxy"] = out["cost"]["q1"]["input_tokens"]
    json.dump(out, sys.stdout, indent=1)


if __name__ == "__main__":
    main(*sys.argv[1:4])
