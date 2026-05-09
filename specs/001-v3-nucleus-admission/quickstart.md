# Quickstart: Bolt-v3 Nucleus Admission Audit

## Report-only run

```bash
just verify-bolt-v3-nucleus-admission
```

Expected on current `main`: command exits successfully and reports Bolt-v3 as
blocked with evidence.

## Strict run

```bash
python3 scripts/verify_bolt_v3_nucleus_admission.py --strict
```

Expected on current `main`: command exits nonzero and reports the same blockers
as report-only mode.

## Self-tests

```bash
python3 scripts/test_verify_bolt_v3_nucleus_admission.py
```

Expected: tests cover report-only status, strict status, positive failing
fixtures, allowed concrete fixture contexts, invalid waivers, and scan-universe
proof.

## Promotion Follow-Up

After the reported blockers are retired, a separate change may wire strict mode
into required CI. That follow-up must prove required CI fails if a nucleus
blocker reappears.
