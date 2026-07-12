# Change-Amplification Baseline Receipt

## Receipt identity

- Planning base: `9f3b13f4c6ae937be69cfb9c44fae409d268ef30`
- Subject: historical raw churn used to motivate issue #1016 and the CI-Python roadmap
- Accounting boundary: each PR is measured from its exact GitHub base SHA to exact head SHA; merge SHA is identity evidence only
- Status: historical evidence and provisional calibration input, not a gate or acceptance requirement

## Reproducible raw method

For each PR, GitHub PR metadata supplied base, head, and merge SHAs. Raw textual churn and touched paths were cross-checked with:

```text
gh pr view <PR> --json baseRefOid,headRefOid,files
git diff --no-renames --numstat <BASE_SHA> <HEAD_SHA> -- <SELECTED_PATHS>
git diff --no-renames --unified=0 <BASE_SHA> <HEAD_SHA> -- <SELECTED_PATHS>
```

Per-row textual lines are the sum of numeric added and deleted fields. Path count is the number of distinct numstat rows for that PR. Cross-PR unique paths are the set union of those rows. The method deliberately uses `--no-renames`.

Accounting rules:

- Opaque binaries count as touched paths and retain blob/byte evidence, but contribute zero textual lines.
- Generated files remain in raw churn. They may be labeled derived only when generator, canonical input, command, and input/output digest provenance exist.
- Moves remain raw churn unless a reproducible checked-in span map with before/after digests exists. No such map exists for #1309.
- Raw churn does not distinguish semantic value from mechanical or derived bytes. Any verifier/test subtotal is reported separately and is not subtracted from raw totals.
- Summed path touches count a path once per PR; unique-path totals count the union once.

## Behavioral PR rows

| PR | Base SHA | Head SHA | Merge SHA | Raw textual lines | Paths | Supported note |
| --- | --- | --- | --- | ---: | ---: | --- |
| #1151 | `638a48630cd2bd02f250fa6003f85d2cd7f5927b` | `82d5133bb99f4be429bcfd10e92f5baee36c2e2b` | `ee24951fee5a818e97e754ceb7749c37c68fa2a0` | 171 | 11 | Nominal-setting aggregate member; 18 direct operational-value churn lines |
| #1186 | `968755d067791ac60fb0ee151d9ef9e101f92d83` | `73ba0939028d2549f5f31b13f1ae8b9501ac2131` | `5282c3178dfb868dca30123a9d14e06d3c42eb6d` | 51 | 2 | Nominal-setting aggregate member; 24 direct operational-value churn lines |
| #1297 | `fb03ab8b170c948c3f12ea4da4bc81d304300442` | `d0f7b083b1081b5605b8afe23864caa635accc56` | `552d42aa1989dfedf1fea3e64f6ae4a5661de1f3` | 87 | 6 | Nominal-setting aggregate member; 4 direct operational-value churn lines |
| #1290 | `fb372a7667b17dcebad9f4590291da90b85399a6` | `bf09d5bdd7ea350786a7fa9c0fe1ec9a6a1fd6bc` | `e29950145ad4f8ebb8dbad54b3d7ce2ac406ec30` | 2,094 | 7 | Verifier/test subtotal: 566 |
| #1173 | `968755d067791ac60fb0ee151d9ef9e101f92d83` | `a28b7a0a60d2ef238bbdc94b072d1f4abf0c43fe` | `619996ae4e32a61652c218bb8ae647bdb98f52c2` | 2,058 | 12 | Verifier/test subtotal: 1,264 (61.42%) |
| #1309 | `ca0d9d2440caa7ecf77fb27f38eff88a7bb62e23` | `66546c0fb31574f77bae011640c58bcade2ebe7c` | `951e0df5619b634d79ed474783d19d299ef1c0ee` | 33,131 | 22 | Relocation reported separately; no reproducible move map |

Six distinct behavioral PRs were examined. The five non-relocation rows (#1151, #1186, #1297, #1290, #1173) total 4,461 raw textual lines across 26 unique paths. The relocation row #1309 is 33,131 raw textual lines over 22 paths. The union across all six is 44 unique paths.

## Supported central cluster

The supported 26,667-line central #1016 cluster at the planning base is exactly:

| Path | Lines |
| --- | ---: |
| `scripts/verify_ci_workflow_hygiene.py` | 11,426 |
| `scripts/test_verify_ci_workflow_hygiene.py` | 12,966 |
| `scripts/ci_workflow_hygiene_test_helpers.py` | 2,149 |
| `scripts/test_rust_verification_decoupling.py` | 126 |
| **Total** | **26,667** |

This enumeration is reproducibility evidence only. It does not define policy ownership, authorize deletion, or change the receipt's interpretation limits.

## Supported nominal subtotal

The nominal aggregate contains only #1151, #1186, and #1297:

| Measure | Supported value |
| --- | ---: |
| Direct operational-value churn | 46 |
| Raw textual changed lines | 309 |
| Summed path touches | 19 |
| Unique paths | 16 |

Direct operational-value churn counts additions plus deletions, so one replacement counts as two lines. These counts are textual selectors, not semantic-fact counts. The subtotal is not added to the five-PR raw total because its three rows are already included there. The implied raw-line/direct-value-churn ratio is a provisional review signal only; representation amplification requires fact-to-representation edge records and is not inferred from raw LOC.

### Exact selector table

| PR | Selected paths and changed material | Count |
| --- | --- | ---: |
| #1151 | All changed lines for three `retention-days: 30` to `14` assignments in `.github/workflows/ci.yml` (6); `max_lookback_age_seconds` and `retention_days` in `ci/chainlink-reference-fixture-capture-provenance.toml` (4); those two keys plus reuse-bound and capture-provenance ceilings in `ci/github-actions-runners.toml` (8) | 18 |
| #1186 | Entire changed hunk in `ci/github-actions-runners.toml`: 12 additions plus 12 deletions over 12 keys under four `cargo_build_jobs` tables | 24 |
| #1297 | Entire `.mergify.yml` changed hunk for `batch_size.max` and `batch_max_failure_resolution_attempts`: 2 additions plus 2 deletions | 4 |
| #1290 | Whole-file additions plus deletions for `scripts/verify_ci_workflow_hygiene.py` (229+25) and `scripts/test_verify_ci_workflow_hygiene.py` (281+31) | 566 |
| #1173 | Whole-file additions plus deletions for `scripts/verify_ci_workflow_hygiene.py` (295+75) and `scripts/test_verify_ci_workflow_hygiene.py` (753+141) | 1,264 (61.42% of 2,058) |

The #1290 and #1173 figures are path-role subtotals only; they do not prove every selected line is duplicated or represents the same semantic fact.

## Positive comparators

| PR | Base SHA | Head SHA | Merge SHA | Raw textual lines | Paths |
| --- | --- | --- | --- | ---: | ---: |
| #1146 | `ae9394092cbb329eaba9c10bbba99a1ab576da61` | `be86e0b907791de950074314ddd580a037086bac` | `d72eb95766ada919d0a6bbefb608b49c089ba4ea` | 10 | 1 |
| #1292 | `fb372a7667b17dcebad9f4590291da90b85399a6` | `b5a9a6ddae4bc2e1e4208d8f03b462fbd3ec0bf1` | `6b99021eb8f97c8974bead6a316e7bf39b3a9907` | 27 | 2 |

These comparators remain separate from behavioral totals. They illustrate low raw touch surface; they do not by themselves prove semantic adequacy.

## Interpretation limits

These observations establish historical touch surface, not causal ownership or a calibrated acceptance threshold. Numerical line, file, ratio, percentage, and rolling-median figures remain provisional review signals until a reproducible calibration set includes canonical facts, manually maintained representation edges, derived outputs, killed mutations, timing, RSS, and cost. No value in this receipt authorizes deletion, weakening, an exception, or production cutover.
