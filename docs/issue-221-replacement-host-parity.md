# Issue 221: Current Launch Decision

## 1. Current answer

**Not approved for trusted launch on the existing host.**

## 2. Evidence

- The original production host hit real root-volume exhaustion (`ENOSPC`) and the forensic clone was
  captured at `100%` used.
- The incident was not confined to one surface: the forensic clone showed large root-side write
  growth across `/var/logs`, `/var/log`, and `/var/lib/amazon/ssm`, not just one misconfigured
  directory.
- The forensic record includes host-damage indicators, not just full disk:
  truncated journal and at least one root-level path returning `Structure needs cleaning`.
- The current live host is still a bad control-plane citizen: `aws ssm send-command` on
  `i-08dee6aefe9a5b02c` still fails immediately with `ResponseCode=1` and empty stdout/stderr,
  even though the instance reports `PingStatus=Online`.
- `#222` proved the old host baseline was root-only, lacked `WorkingDirectory`, lacked a dedicated
  service user, and carried root-owned runtime/config paths. That is the opposite of a clean,
  trusted baseline for in-place repair.
- `#223` proved what operators must be able to observe for approval; the current host does not meet
  that bar because a core remote-control surface is still broken.
- `#224` proved the merged `#215` host/storage/service baseline can be reproduced cleanly on a fresh
  EC2 instance, which removes the main reason to prefer risky in-place surgery on a known-damaged
  box.
- `#224` also proved the remaining blocker is now a concrete runtime-startup issue (`#225`), not a
  need to salvage the damaged host.

## 3. Exact repair path if repairable

- None within this launch job / timebox. I do **not** have an evidence-backed in-place repair path
  that restores trust in the existing host enough for launch.

## 4. Exact stop condition if not repairable

- Stop in-place repair immediately because the host has already crossed the trust boundary from
  “misconfigured” into “operationally damaged”:
  root-volume exhaustion, journal truncation, filesystem-cleanliness concerns, and a still-broken
  `RunShellScript` control path.
- Do not treat “instance is still running” or “SSM PingStatus is Online” as evidence of trust.
- Do not launch from this host unless someone explicitly accepts that they are launching from a box
  whose remote-control path is still broken after a filesystem-damage incident.

## 5. What should happen next

1. Keep `#221` as the single control issue and decision surface.
2. Treat the current host as reference-only evidence, not as a launch candidate.
3. Continue on the fresh candidate path already established in `#224`, because that is the only path
   that preserves the full production lane by default while removing the proven-negative part
   (the damaged root filesystem and host state).
4. Resolve `#225`, then rerun candidate validation from the real production network identity
   boundary before making the final launch/cutover decision in `#221`.
