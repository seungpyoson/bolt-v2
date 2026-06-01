# External Review Import Template

Use this template for each external model review obtained outside this sandbox. A review only counts toward the gate if the reviewed packet matches the current packet in `reviews.md` or the drift is explicitly accepted and harmless.

## Required Metadata

- Provider:
- Model or product:
- Review date:
- Reviewer route/tool:
- Reviewed repo:
- Reviewed branch:
- Reviewed commit SHA:
- Reviewed files:
- Prompt used:
- Source packet byte count, if available:
- Source packet hash or file hashes, if available:

## Verdict

One of:

- `APPROVE`
- `REQUEST_CHANGES`

## Findings

List blocking findings first. For every blocking finding, include:

- File/path:
- Line or section:
- Evidence:
- Risk:
- Required design change:

If there are no blocking findings, write:

```text
No blocking findings.
```

## Nonblocking Notes

List nonblocking hardening notes separately. These do not prevent issue creation unless they contradict an accepted fact or approval criterion.

## Acceptance Check

Before recording this review as accepted:

- The verdict is `APPROVE`.
- The reviewed files match `goals/production-kill-switch/reviews.md`.
- The reviewed design is current or every drift is documented.
- Claude and Gemini reviews are present before satisfying the mandatory quorum.
- The review does not rely on a non-source-bearing summary unless the gate is explicitly changed.
