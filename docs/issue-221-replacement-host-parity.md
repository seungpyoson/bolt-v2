# Issue 221: Replacement-Host Parity Gate

## 1. Current answer

**Not approved: a rebuilt host is not yet approved as a production-equivalent replacement environment.**

## 2. Parity rule

Rebuild/cutover approval must preserve the full production lane first, end to end: AWS/network
identity, host/OS behavior, binary/runtime behavior, config/secrets/startup behavior, actual
trading behavior, and operator/rollback behavior. Only explicitly approved deltas may differ.
Anything omitted is a regression until proven otherwise.

## 3. What has already been proven

- `#215` is merged, so the root-volume remediation baseline is defined in `main`: dedicated data
  volume layout, `WorkingDirectory=/srv/bolt-v2`, `User=bolt`, absolute runtime write paths, and
  capped journald.
- `#222` captured the current production/forensic host baseline that matters for parity review:
  AWS identity, EIP/network boundary, root-only pre-remediation mount layout, systemd/journald
  state, package/sysctl/limits/timer context, deployed artifact identity, and the rendered live
  lane config recovered from the forensic clone.
- `#223` defined the minimum operator-visible monitoring contract required for approval: control
  plane, storage/mount health, service/restart state, config/secret resolution, reference health,
  selector health, reconnect-storm visibility, strategy readiness, and audit backlog health.
- `#224` proved that the merged `#215` host/storage/service baseline can be provisioned on a fresh
  EC2 instance with the same AMI / instance family / subnet / AZ / SG / IAM profile boundary.
- `#224` also proved that a non-EIP candidate run is insufficient for trading parity: the host can
  be provisioned cleanly without yet proving operation from the real production network identity.
- `#224` further proved that the first fresh candidate still failed actual runtime startup: Binance
  did not connect, the trader did not start, and the host was therefore not approved.

## 4. Remaining blockers

- `#225`: the current Binance startup path still blocks trader startup on the candidate run
  (`HTTP 400 Bad Request` / `Invalid X-MBX-APIKEY header` on the SBE endpoint).
- The candidate run in `#224` did not validate from the full real production network identity
  boundary, including the whitelisted EIP/source-IP path that counterparties actually see.
- Because of those two points, full trading parity is still unproven: feed connectivity,
  reference health, trader start, strategy readiness, counterparty acceptance, and latency-sensitive
  behavior from the real production boundary are not yet established.

## 5. Next actions

1. Resolve `#225` so the Binance startup path succeeds under the real production credential and
   network-identity boundary.
2. Re-run the candidate-host exercise from the full production boundary, including the production
   EIP / source-IP path and every behavior tied to that network identity.
3. Re-verify the entire lane on that candidate in one pass: AWS/network identity, host/OS state,
   binary/runtime identity, config/secret resolution, startup validation, selector/reference
   behavior, actual trader start, strategy readiness, operator visibility, and rollback viability.
4. Update `#221` with the exact result of that rerun and make the rebuild/cutover decision here,
   not in side branches.

## 6. Approval criteria

Rebuild/cutover is approved only when all of the following are true:

- The candidate host preserves the real production AWS/network identity boundary, including the EIP
  / source-IP behavior and any counterparty allowlist effects that depend on it.
- The candidate host preserves the required host/OS/system behavior from the production lane, or
  every delta is explicit, justified, and accepted.
- The candidate host runs the intended artifact and runtime shape from `main`, with the intended
  rendered config and successful SSM-based secret resolution.
- Startup completes from the real production boundary with healthy feed connectivity, healthy
  reference behavior, healthy selector behavior, trader start, and strategy readiness.
- Operators can observe healthy vs unhealthy state and perform rollback without silent failures.
- No remaining blocker in this approval path, including `#225`, still prevents full production
  parity.
