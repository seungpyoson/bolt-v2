// Self-tests for the host-health.html inline viewer script (CLASS D, #884).
//
// Run with:  node scripts/test_host_health_viewer.mjs
//
// Dependency-free: reads the REAL host-health.html, extracts its inline
// <script> body, and evaluates it inside a Node `vm` context backed by a tiny
// DOM stub (NO jsdom, NO npm). The script's top-level `function`/`const`
// declarations land on the vm context's global object, so the pure functions
// (parseJsonl, diskFreePct, diskBadgeClass, bannerReasons, ...) are exercised
// as the actually-shipped code — not a copy.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
// Allow `node scripts/test_host_health_viewer.mjs <path-to-html>` so the same
// test can run against a pre-fix HEAD copy for the fail-before evidence.
const htmlPath = process.argv[2]
  ? resolve(process.argv[2])
  : resolve(here, "..", "host-health.html");

function extractInlineScript(html) {
  // The viewer has exactly one inline <script>...</script> (no src attr).
  const match = html.match(/<script>([\s\S]*?)<\/script>/);
  if (!match) throw new Error(`no inline <script> found in ${htmlPath}`);
  return match[1];
}

// --- Minimal DOM stub: enough for the script to run render() to completion. ---
function makeNode() {
  const node = {
    textContent: "",
    innerHTML: "",
    className: "",
    hidden: false,
    style: {},
    children: [],
    setAttribute() {},
    getAttribute() {
      return null;
    },
    appendChild(child) {
      this.children.push(child);
      return child;
    },
    addEventListener() {}
  };
  return node;
}

function makeDocument() {
  return {
    getElementById() {
      return makeNode();
    },
    createElement() {
      return makeNode();
    },
    createElementNS() {
      return makeNode();
    },
    createTextNode(text) {
      const node = makeNode();
      node.textContent = text;
      return node;
    }
  };
}

function buildContext() {
  const document = makeDocument();
  const sandbox = {
    document,
    window: {},
    console,
    // FileReader is referenced inside the change handler only; a stub is enough
    // for the script body to evaluate without throwing.
    FileReader: class {
      readAsText() {}
    }
  };
  sandbox.window = sandbox;
  return vm.createContext(sandbox);
}

function loadViewer() {
  const html = readFileSync(htmlPath, "utf8");
  const scriptText = extractInlineScript(html);
  const context = buildContext();
  // runInContext executes the full script (including the trailing render()).
  // Top-level function/const declarations become own properties of the context.
  vm.runInContext(scriptText, context, { filename: "host-health.inline.js" });
  return context;
}

// --- tiny assert harness -----------------------------------------------------
let passed = 0;
const failures = [];

function check(name, fn) {
  try {
    fn();
    passed += 1;
    console.log(`ok   - ${name}`);
  } catch (error) {
    failures.push({ name, error });
    console.log(`FAIL - ${name}: ${error.message}`);
  }
}

function assertEqual(actual, expected, message) {
  if (!Object.is(actual, expected)) {
    throw new Error(`${message || "values differ"}: expected ${expected}, got ${actual}`);
  }
}

function assertTrue(value, message) {
  if (value !== true) {
    throw new Error(`${message || "expected true"}, got ${value}`);
  }
}

function requireFn(context, name) {
  const fn = context[name];
  if (typeof fn !== "function") {
    throw new Error(`viewer did not expose function ${name} (got ${typeof fn})`);
  }
  return fn;
}

// --- tests -------------------------------------------------------------------
const ctx = loadViewer();

// D1: a non-finite disk free% must be a degraded (vio) badge, never green.
check("D1 diskBadgeClass(NaN) -> vio", () => {
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  assertEqual(diskBadgeClass(NaN), "vio", "NaN must be degraded");
});
check("D1 diskBadgeClass(undefined) -> vio", () => {
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  assertEqual(diskBadgeClass(undefined), "vio", "undefined must be degraded");
});
check("D1 diskBadgeClass keeps numeric thresholds", () => {
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  assertEqual(diskBadgeClass(5), "red");
  assertEqual(diskBadgeClass(15), "amb");
  assertEqual(diskBadgeClass(50), "grn");
});

// D3: card free% is derived from df-convention used_pct (free + used == 100).
check("D3 diskFreePct({used_pct:12.5}) -> 87.5", () => {
  const diskFreePct = requireFn(ctx, "diskFreePct");
  assertEqual(diskFreePct({ used_pct: 12.5 }), 87.5);
});
check("D3 diskFreePct({used_pct:null}) -> null", () => {
  const diskFreePct = requireFn(ctx, "diskFreePct");
  assertEqual(diskFreePct({ used_pct: null }), null);
});
check("D3 diskFreePct(undefined disk) -> null", () => {
  const diskFreePct = requireFn(ctx, "diskFreePct");
  assertEqual(diskFreePct(null), null);
});

// D5: a leading UTF-8 BOM must not drop the first record.
check("D5 parseJsonl strips BOM, yields 1 record", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const result = parseJsonl("﻿{\"schema_version\":2}");
  assertEqual(result.records.length, 1, "BOM record must survive");
  assertEqual(result.errors.length, 0, "no parse error expected");
  assertEqual(result.records[0].schema_version, 2);
});

// D6: bare number/array/null are valid JSON but NOT records.
check("D6 parseJsonl rejects non-object lines", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const result = parseJsonl("42\n[1,2]\nnull");
  assertEqual(result.records.length, 0, "no fake records");
  assertEqual(result.errors.length, 3, "each bad line gets an error");
  assertTrue(result.errors.every(e => /not a JSON object/.test(e)), "descriptive error");
});
check("D6 parseJsonl still accepts real (schema-2) objects", () => {
  // Use minimal VALID schema-2 sample rows rather than arbitrary {"a":1} blobs:
  // for a health viewer, "accepts an object" is only meaningful when the object
  // is a plausible sample row. Shape acceptance is the point here; the rendered
  // outcome for these rows is asserted by the Fix B render-level tests.
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const result = parseJsonl("{\"schema_version\":2,\"service\":{\"active_state\":\"active\"}}\n{\"schema_version\":2,\"disk\":{\"used_pct\":20}}");
  assertEqual(result.records.length, 2);
  assertEqual(result.errors.length, 0);
  assertEqual(result.records[0].schema_version, 2);
  assertEqual(result.records[1].disk.used_pct, 20);
});

// D2: standing degraded state in the latest sample produces a banner reason
// even without an in-file increase.
check("D2 bannerReasons flags active_state=failed", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: { active_state: "failed" } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.length > 0, "failed state must raise a reason");
  assertTrue(reasons.some(r => /active_state=failed/.test(r)), "reason names the failure");
});
check("D2 bannerReasons flags standing n_restarts>0", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: { active_state: "active", n_restarts: 3 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /auto-restarted 3/.test(r)), "standing restart count flagged");
});
check("D2 bannerReasons flags failure result", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: { active_state: "active", result: "signal" } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /result=signal/.test(r)), "failure result flagged");
});
check("D2 bannerReasons clean record -> no reasons", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // A genuinely clean record now includes a present disk block: an absent/null
  // disk is itself a degraded signal (E3) that the render path already shows as
  // a violet chip and the banner now surfaces. "Clean" means service healthy
  // AND disk present.
  const latest = { schema_version: 2, oom_killed: null, disk: { used_pct: 20 }, service: { active_state: "active", n_restarts: 0, result: "success" } };
  const reasons = bannerReasons(latest, [latest]);
  assertEqual(reasons.length, 0, "a clean record must not raise a banner");
});

// D4: cumulative cgroup oom latch vs fresh in-window increase are distinguishable.
check("D4 bannerReasons: cumulative latch wording", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: { cgroup_oom_kills: 5 } };
  const reasons = bannerReasons(latest, [latest]); // single record -> no increase
  assertTrue(reasons.some(r => /cumulative/.test(r)), "stale latch worded as cumulative");
  assertTrue(!reasons.some(r => /increased to/.test(r)), "no fresh-increase claim on a latch");
});
check("D4 bannerReasons: fresh increase wording", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const records = [
    { service: { cgroup_oom_kills: 0 } },
    { service: { cgroup_oom_kills: 2 } }
  ];
  const reasons = bannerReasons(records[records.length - 1], records);
  assertTrue(reasons.some(r => /increased to 2 in this file/.test(r)), "fresh kill flagged distinctly");
});
check("Fix B bannerReasons detects top-level cgroup OOM increase with service null", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const records = [
    { oom_killed: true, service: null, cgroup_oom_kills: 5, schema_version: 2, disk: { used_pct: 1 } },
    { oom_killed: true, service: null, cgroup_oom_kills: 9, schema_version: 2, disk: { used_pct: 1 } }
  ];
  const reasons = bannerReasons(records[records.length - 1], records);
  assertTrue(
    reasons.some(r => /cgroup oom_kill increased to 9/.test(r)),
    "top-level cgroup increase must raise the fresh-increase banner reason"
  );
  assertTrue(
    !reasons.some(r => /cumulative/.test(r)),
    "fresh top-level increase must not be downgraded to cumulative wording"
  );
});

// === PR #886 review round 2 ===================================================

// Item 6: ANY non-success Result is degraded (not just an allow-list). A
// timeout/exit-code/resources result with active_state="active" must raise a
// banner reason. Pre-fix FAILURE_RESULTS omitted these.
check("R2-6 isFailureResult predicate", () => {
  const isFailureResult = requireFn(ctx, "isFailureResult");
  assertEqual(isFailureResult("success"), false, "success is healthy");
  assertEqual(isFailureResult(""), false, "empty result is not a failure signal");
  assertEqual(isFailureResult(null), false, "null result is not a failure signal");
  assertEqual(isFailureResult("timeout"), true, "timeout is degraded");
  assertEqual(isFailureResult("exit-code"), true, "exit-code is degraded");
  assertEqual(isFailureResult("resources"), true, "resources is degraded");
  assertEqual(isFailureResult("protocol"), true, "protocol is degraded");
  assertEqual(isFailureResult("assert"), true, "assert is degraded");
});
check("R2-6 bannerReasons flags result=timeout under active", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: { active_state: "active", result: "timeout" } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /result=timeout/.test(r)), "timeout result raises a reason");
});

// Item 5: serviceBadge must never render green when the latest sample is
// degraded, and must never blank out when service data is missing.
check("R2-5 serviceBadge active+oom_killed -> red, not green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ oom_killed: true, service: { active_state: "active", sub_state: "running" } });
  assertTrue(/badge red/.test(html), "active-after-OOM must be red");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});
check("R2-5 serviceBadge active+failure-result -> red", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: { active_state: "active", sub_state: "running", result: "timeout" } });
  assertTrue(/badge red/.test(html), "active with a failure result must be red");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});
check("R2-5 serviceBadge active+n_restarts>0 -> amb", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: { active_state: "active", sub_state: "running", n_restarts: 2 } });
  assertTrue(/badge amb/.test(html), "active-after-restart must be amber");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});
check("R2-5 serviceBadge clean active -> green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  // A genuinely clean record is schema-2 with no collector errors AND a healthy
  // service. Without schema_version the record is degraded at the record level
  // (Fix A), so it would no longer be green — the sanity input must be clean.
  const html = serviceBadge({ schema_version: 2, oom_killed: null, service: { active_state: "active", sub_state: "running", n_restarts: 0, result: "success" } });
  assertTrue(/badge grn/.test(html), "a clean active service is still green");
});
check("R2-5 serviceBadge no service data -> degraded, never blank", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: null });
  assertTrue(/badge vio/.test(html), "missing service is a degraded badge");
  assertTrue(/service unknown/.test(html), "labelled, never blank");
});
check("R2-5 bannerReasons flags missing service data", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { service: null };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /service status unavailable/.test(r)), "no-service sample raises banner");
});

// Item 7: OOM banner wording must name its evidence source. An OOM derived from
// result=signal + cgroup counter must NOT be mislabelled as the authoritative
// systemd Result=oom-kill.
check("R2-7 OOM from signal+cgroup names result and count", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { oom_killed: true, service: { active_state: "active", result: "signal", cgroup_oom_kills: 3 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /result=signal/.test(r) && /oom_kill=3/.test(r)), "cgroup-corroborated wording");
  assertTrue(!reasons.some(r => /^systemd Result=oom-kill \(authoritative\)$/.test(r)), "not mislabelled authoritative");
});
check("R2-7 authoritative oom-kill keeps authoritative wording", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { oom_killed: true, service: { active_state: "failed", result: "oom-kill", cgroup_oom_kills: 1 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /authoritative/.test(r)), "systemd oom-kill stays authoritative");
});
// Round-4 coherence: when systemctl stalled (service null) but a cgroup OOM
// fired, the top-level count must surface in the banner instead of a
// self-contradictory "oom_kill=0". Pre-fix the count was read only from the
// (null) service block, so an OOM with a known count rendered as count 0.
check("R4 OOM count surfaces from record top level when service is null", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { oom_killed: true, service: null, cgroup_oom_kills: 9 };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /oom_kill=9/.test(r)), "real top-level count surfaces");
  assertTrue(!reasons.some(r => /oom_kill=0/.test(r)), "must not show a contradictory 0");
});
check("R4 OOM count back-compat: nested-only count still read", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { oom_killed: true, service: { active_state: "active", result: "signal", cgroup_oom_kills: 4 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /oom_kill=4/.test(r)), "nested count read when top-level field absent");
});
check("Fix H cgroupOomCount never under-reports conflicting top and nested counts", () => {
  const cgroupOomCount = requireFn(ctx, "cgroupOomCount");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = {
    oom_killed: true,
    cgroup_oom_kills: 0,
    service: { active_state: "active", sub_state: "running", result: "signal", cgroup_oom_kills: 9 },
    schema_version: 2,
    disk: { used_pct: 1 }
  };
  const reasons = bannerReasons(latest, [latest]);
  assertEqual(cgroupOomCount(latest), 9, "conflicting finite counts surface the larger value");
  assertTrue(!reasons.some(r => /oom_kill=0\b/.test(r)), "banner must not report contradictory zero OOM count");
});

// === PR #886 review round 3 (fail-closed hardening E1/E2/E4/E3/F/G) ==========

// E1: active/exited (or any active sub_state != "running") is degraded, never
// green: systemd still calls the unit active but its main process has exited.
// Pre-fix the active branch went straight to grn on a 0-restart success.
check("E1 serviceBadge active/exited -> amb, not green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: { active_state: "active", sub_state: "exited", result: "success", n_restarts: 0 } });
  assertTrue(/badge amb/.test(html), "active/exited must be amber");
  assertTrue(!/badge grn/.test(html), "active/exited must NOT be green");
});
check("E1 serviceBadge active/running clean -> green (unchanged)", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ schema_version: 2, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } });
  assertTrue(/badge grn/.test(html), "clean active/running is still green");
});

// E2: an OOM kill forces RED for ANY active_state. Pre-fix the oom_killed check
// lived INSIDE the active branch, so an OOM victim now inactive rendered violet
// and one now activating rendered amber.
check("E2 serviceBadge oom_killed + inactive -> red (pre-fix: vio)", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ oom_killed: true, service: { active_state: "inactive", sub_state: "dead", result: "signal" } });
  assertTrue(/badge red/.test(html), "OOM victim now inactive must be red");
  assertTrue(!/badge vio/.test(html), "must NOT be violet");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});
check("E2 serviceBadge oom_killed + activating -> red (pre-fix: amb)", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ oom_killed: true, service: { active_state: "activating", sub_state: "start", result: "signal" } });
  assertTrue(/badge red/.test(html), "OOM victim now activating must be red");
  assertTrue(!/badge amb/.test(html), "must NOT be amber");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});

// E4: a PRESENT but non-finite n_restarts in the green-eligible path is suspect
// restart data -> degraded (amber), never green. Pre-fix Number("garbage")=NaN
// failed the `> 0` test and fell through to grn.
check("E4 serviceBadge non-finite n_restarts -> not green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: { active_state: "active", sub_state: "running", result: "success", n_restarts: "garbage" } });
  assertTrue(/badge amb/.test(html), "suspect restart data must be amber");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});
check("E4 serviceBadge null n_restarts stays green-eligible", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ schema_version: 2, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: null } });
  assertTrue(/badge grn/.test(html), "null/absent restart field may stay green");
});

// E3: a null/absent disk block (sampler could not stat the catalog path) must
// raise a banner reason, parallel to "service status unavailable". Pre-fix
// bannerReasons emitted nothing for a null disk (only a quiet violet chip).
check("E3 bannerReasons flags null disk", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { disk: null, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /disk status unavailable/.test(r)), "null disk raises a banner reason");
});
check("E3 bannerReasons: present disk does not raise the disk reason", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { schema_version: 2, disk: { used_pct: 20 }, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(!reasons.some(r => /disk status unavailable/.test(r)), "a present disk must not raise the reason");
});

// F: min/max must come from an accumulator loop, not Math.min(...values). The
// spread overflows V8's argument limit at ~110K+ elements and throws RangeError,
// aborting render() and blanking the UI. arrayExtent has no such ceiling.
check("F arrayExtent handles 200k elements without throwing", () => {
  const arrayExtent = requireFn(ctx, "arrayExtent");
  const big = new Array(200000);
  for (let i = 0; i < big.length; i += 1) big[i] = i;
  // Pre-fix evidence: Math.min(...big) throws here. arrayExtent must not.
  let threw = false;
  let extent;
  try {
    extent = arrayExtent(big);
  } catch (error) {
    threw = true;
  }
  assertTrue(!threw, "arrayExtent must not throw on a huge array");
  assertEqual(extent.lo, 0, "min of 0..199999 is 0");
  assertEqual(extent.hi, 199999, "max of 0..199999 is 199999");
});
check("F Math.min(...arr) DOES overflow at 200k (pre-fix failure mode)", () => {
  // Anchors WHY the fix is load-bearing: the spread the fix removed throws here.
  const big = new Array(200000).fill(1);
  let threw = false;
  try {
    Math.min(...big);
  } catch (error) {
    threw = true;
  }
  assertTrue(threw, "Math.min(...big) overflows the call-argument limit");
});
check("F drawTimeSeries survives 200k records via the real render path", () => {
  // Integration evidence that the SHIPPED drawTimeSeries (not just the extracted
  // helper) no longer spreads a huge array. Pre-fix this throws RangeError from
  // Math.min(...values) and aborts render(); post-fix it runs to completion.
  const drawTimeSeries = requireFn(ctx, "drawTimeSeries");
  const records = new Array(200000);
  for (let i = 0; i < records.length; i += 1) {
    records[i] = { sampled_at: "2026-06-22T00:00:00Z", process: { rss_bytes: i } };
  }
  const container = ctx.document.createElement("div");
  let threw = false;
  try {
    drawTimeSeries(container, {
      records,
      unit: "bytes",
      formatter: ctx.formatBytes,
      series: [{ name: "RSS", color: "var(--blu)", value: record => record.process && record.process.rss_bytes }]
    });
  } catch (error) {
    threw = true;
  }
  assertTrue(!threw, "drawTimeSeries must not throw on 200k records");
  assertTrue(container.children.length > 0, "it appended chart output, did not abort early");
});
check("F arrayExtent matches inline min/max on a small input", () => {
  const arrayExtent = requireFn(ctx, "arrayExtent");
  const extent = arrayExtent([3, -2, 7, 0, 5]);
  assertEqual(extent.lo, -2, "min preserved on a normal-size input");
  assertEqual(extent.hi, 7, "max preserved on a normal-size input");
});

// G: restartIncreased must apply the same Number.isFinite finite-guard as its
// sibling counterIncreased. Pre-fix, a non-numeric n_restarts was NOT skipped:
// it set previous=Number("x")=NaN, masking the very next comparison. With a
// non-numeric value sitting BETWEEN two valid samples (5, "x", 6), the real
// 5->6 increase is hidden because the pre-fix code compares 6 against NaN
// instead of against the last VALID value 5. Post-fix delegates to
// counterIncreased, which `continue`s past the non-numeric and keeps previous=5,
// so 6>5 is detected.
check("G restartIncreased: non-numeric between valids must not mask the increase", () => {
  const restartIncreased = requireFn(ctx, "restartIncreased");
  const records = [
    { service: { n_restarts: 5 } },
    { service: { n_restarts: "x" } },
    { service: { n_restarts: 6 } }
  ];
  assertEqual(restartIncreased(records), true, "non-numeric must not poison the next comparison");
});
check("G restartIncreased: clean increasing sequence -> true", () => {
  const restartIncreased = requireFn(ctx, "restartIncreased");
  const records = [
    { service: { n_restarts: 0 } },
    { service: { n_restarts: 1 } }
  ];
  assertEqual(restartIncreased(records), true, "increasing restarts detected");
});
check("G restartIncreased: flat sequence -> false", () => {
  const restartIncreased = requireFn(ctx, "restartIncreased");
  const records = [
    { service: { n_restarts: 2 } },
    { service: { n_restarts: 2 } }
  ];
  assertEqual(restartIncreased(records), false, "no increase on a flat sequence");
});

// === Round 4 fail-closed hardening ===========================================

check("Fix 4 diskFreePct rejects out-of-range used_pct", () => {
  const diskFreePct = requireFn(ctx, "diskFreePct");
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  assertEqual(diskFreePct({ used_pct: -5 }), null, "negative used_pct is invalid");
  assertEqual(diskBadgeClass(diskFreePct({ used_pct: -5 })), "vio", "invalid disk metric must be violet");
  assertEqual(diskFreePct({ used_pct: 150 }), null, "over-100 used_pct is invalid");
  assertEqual(diskFreePct({ used_pct: 40 }), 60, "normal used_pct still maps to free pct");
});

check("Fix 8 serviceBadge OOM wins over missing service data", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ service: { active_state: null, sub_state: null, result: "oom-kill" }, oom_killed: true });
  assertTrue(/badge red/.test(html), "OOM must render red");
  assertTrue(!/badge vio/.test(html), "OOM must not be hidden as unknown");
});

check("Fix 9 bannerReasons flags unavailable disk metric", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const metricMissing = { disk: { used_pct: null }, service: { active_state: "active", sub_state: "running", result: "success" } };
  const diskMissing = { disk: null, service: { active_state: "active", sub_state: "running", result: "success" } };
  assertTrue(
    bannerReasons(metricMissing, [metricMissing]).some(r => /disk metric unavailable/.test(r)),
    "present disk with null used_pct raises metric reason"
  );
  assertTrue(
    bannerReasons(diskMissing, [diskMissing]).some(r => /disk status unavailable/.test(r)),
    "null disk still raises status reason"
  );
});

check("Fix 10 bannerReasons flags unexpected schema_version", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const schemaOne = { schema_version: 99, disk: { used_pct: 20 }, service: { active_state: "active", sub_state: "running", result: "success" } };
  const schemaTwo = { schema_version: 2, disk: { used_pct: 20 }, service: { active_state: "active", sub_state: "running", result: "success" } };
  assertTrue(
    bannerReasons(schemaOne, [schemaOne]).some(r => /unexpected schema_version=99/.test(r)),
    "schema v99 raises fail-loud banner reason"
  );
  assertTrue(
    !bannerReasons(schemaTwo, [schemaTwo]).some(r => /unexpected schema_version=2/.test(r)),
    "schema v2 does not raise schema banner reason"
  );
});
check("Fix F bannerReasons surfaces sampler collector errors", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = {
    schema_version: 2,
    errors: ["service: NRestarts malformed: 'x'"],
    disk: { used_pct: 20 },
    service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 }
  };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(
    reasons.some(r => /collector error/.test(r)),
    "sampler collector errors must raise a prominent banner reason"
  );
});
check("Fix G bannerReasons flags missing schema_version", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = {
    disk: { used_pct: 20 },
    service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 }
  };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(
    reasons.some(r => /schema_version missing/.test(r)),
    "missing schema_version must fail closed in the banner"
  );
});

// === PR #886 review round (Fix A: record-level integrity caps the badge) =====
//
// serviceBadge previously computed PURELY from the service block and ignored
// record-level integrity, so a record that bannerReasons simultaneously flags
// (missing schema_version, or a non-empty errors[]) could render a healthy
// GREEN badge — a self-contradictory dashboard. Fix A caps green via the shared
// recordHasIntegrityIssue predicate. These tests are DIFFERENTIAL: they FAIL
// against the pre-fix host-health.html and PASS against the fix.

check("Fix A recordHasIntegrityIssue predicate", () => {
  const recordHasIntegrityIssue = requireFn(ctx, "recordHasIntegrityIssue");
  assertEqual(recordHasIntegrityIssue(null), true, "falsy record is an integrity issue");
  assertEqual(recordHasIntegrityIssue({}), true, "missing schema_version is an integrity issue");
  assertEqual(recordHasIntegrityIssue({ schema_version: null }), true, "null schema_version is an integrity issue");
  assertEqual(recordHasIntegrityIssue({ schema_version: 99 }), true, "unexpected schema_version is an integrity issue");
  assertEqual(recordHasIntegrityIssue({ schema_version: 2, errors: ["boom"] }), true, "non-empty errors[] is an integrity issue");
  assertEqual(recordHasIntegrityIssue({ schema_version: 2, errors: [] }), false, "schema-2 with empty errors is clean");
  assertEqual(recordHasIntegrityIssue({ schema_version: 2 }), false, "schema-2 with no errors field is clean");
});

check("Fix A serviceBadge missing schema_version -> not green (banner contradiction)", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // Healthy-looking service, but the record has NO schema_version. bannerReasons
  // flags it; the badge must not contradict that with green.
  const latest = { service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } };
  const html = serviceBadge(latest);
  assertTrue(!/badge grn/.test(html), "missing-schema record must NOT render green");
  assertTrue(/badge amb/.test(html), "degraded record renders amber");
  assertTrue(
    bannerReasons(latest, [latest]).some(r => /schema_version missing/.test(r)),
    "banner still flags the missing schema_version (no divergence)"
  );
});

check("Fix A serviceBadge non-empty errors[] -> not green (banner contradiction)", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // schema-2, healthy-looking service, but the sampler reported a collector
  // error. The badge must not read green while the banner reports the error.
  const latest = {
    schema_version: 2,
    errors: ["service: NRestarts malformed: 'x'"],
    service: { active_state: "active", sub_state: "running", result: "success", n_restarts: null }
  };
  const html = serviceBadge(latest);
  assertTrue(!/badge grn/.test(html), "errors[]-non-empty record must NOT render green");
  assertTrue(/badge amb/.test(html), "degraded record renders amber");
  assertTrue(
    bannerReasons(latest, [latest]).some(r => /collector error/.test(r)),
    "banner still flags the collector error (no divergence)"
  );
});

check("Fix A serviceBadge unexpected schema_version -> not green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ schema_version: 99, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } });
  assertTrue(!/badge grn/.test(html), "schema!=2 record must NOT render green");
  assertTrue(/badge amb/.test(html), "degraded record renders amber");
});

check("Fix A integrity cap does not over-suppress: clean schema-2 stays green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const html = serviceBadge({ schema_version: 2, errors: [], service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } });
  assertTrue(/badge grn/.test(html), "a fully-clean schema-2 record is still green");
});

check("Fix A integrity cap does not weaken OOM red on a degraded record", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  // No schema_version (integrity issue) AND oom_killed: OOM-red must win, the
  // amber cap must not override the stronger red signal.
  const html = serviceBadge({ oom_killed: true, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } });
  assertTrue(/badge red/.test(html), "OOM red survives the integrity-issue path");
  assertTrue(!/badge grn/.test(html), "must NOT be green");
});

// === PR #886 review round (Fix B: replace vacuous parser tests) ==============
//
// The old D6 acceptance test fed arbitrary objects ({"a":1},{"b":2}) — vacuous
// for a health viewer because it never asserts the RENDERED badge/banner outcome
// for a malformed-shape row. Replaced below with minimal VALID schema-2 samples
// where parser-shape coverage is the point, plus render-level assertions tied to
// Fix A.

check("Fix B parseJsonl accepts minimal valid schema-2 objects", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const result = parseJsonl("{\"schema_version\":2}\n{\"schema_version\":2,\"service\":{\"active_state\":\"active\"}}");
  assertEqual(result.records.length, 2, "two valid schema-2 rows parse as records");
  assertEqual(result.errors.length, 0, "no parse errors on valid rows");
  assertEqual(result.records[0].schema_version, 2);
  assertEqual(result.records[1].service.active_state, "active");
});

check("Fix B render-level: missing-schema healthy-service row is NOT green", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // A malformed-SHAPE row (no schema_version) that nonetheless carries a
  // healthy-looking service. Parser accepts the object; the RENDER must fail
  // closed: no green badge, banner flags it.
  const result = parseJsonl("{\"service\":{\"active_state\":\"active\",\"sub_state\":\"running\",\"result\":\"success\",\"n_restarts\":0}}");
  assertEqual(result.records.length, 1, "the row parses as a record");
  const record = result.records[0];
  assertTrue(!/badge grn/.test(serviceBadge(record)), "missing-schema row must not render green");
  assertTrue(
    bannerReasons(record, [record]).some(r => /schema_version missing/.test(r)),
    "banner flags the missing schema_version"
  );
});

check("Fix B render-level: errors[]-non-empty healthy-service row is NOT green", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const result = parseJsonl("{\"schema_version\":2,\"errors\":[\"collector boom\"],\"service\":{\"active_state\":\"active\",\"sub_state\":\"running\",\"result\":\"success\",\"n_restarts\":0}}");
  assertEqual(result.records.length, 1, "the row parses as a record");
  const record = result.records[0];
  assertTrue(!/badge grn/.test(serviceBadge(record)), "errors[]-non-empty row must not render green");
  assertTrue(
    bannerReasons(record, [record]).some(r => /collector error/.test(r)),
    "banner flags the collector error"
  );
});

// === PR #886 review round (Fix 1: malformed errors shape + mechanical SSOT) ===
//
// A schema-2 record whose `errors` field is PRESENT but NOT an array (e.g. a
// string the sampler serialized by mistake) is a malformed-shape record. Pre-fix
// recordHasIntegrityIssue only checked Array.isArray(errors) && length>0, so a
// non-array errors read as clean: false integrity, GREEN badge, no banner. These
// tests are DIFFERENTIAL: RED against the pre-fix html, GREEN against the fix.
// They also lock the new recordIntegrityReasons SSOT that unifies the badge gate
// and the banner wording.

check("Fix 1 recordIntegrityReasons SSOT: reasons array", () => {
  const recordIntegrityReasons = requireFn(ctx, "recordIntegrityReasons");
  assertTrue(recordIntegrityReasons(null).some(r => /record missing/.test(r)), "falsy record names a reason");
  assertTrue(recordIntegrityReasons({}).some(r => /schema_version missing/.test(r)), "missing schema reason");
  assertTrue(recordIntegrityReasons({ schema_version: 99 }).some(r => /unexpected schema_version=99/.test(r)), "unexpected schema reason");
  assertTrue(recordIntegrityReasons({ schema_version: 2, errors: ["boom"] }).some(r => /reported 1 collector error/.test(r)), "non-empty errors reason");
  // THE Fix 1 case: a non-array errors field.
  assertTrue(recordIntegrityReasons({ schema_version: 2, errors: "boom" }).some(r => /errors field malformed \(expected array, got string\)/.test(r)), "malformed-shape errors reason");
  assertEqual(recordIntegrityReasons({ schema_version: 2, errors: [] }).length, 0, "empty errors array is clean");
  assertEqual(recordIntegrityReasons({ schema_version: 2 }).length, 0, "absent errors is clean");
});

check("Fix 1 recordHasIntegrityIssue flags malformed (non-array) errors", () => {
  const recordHasIntegrityIssue = requireFn(ctx, "recordHasIntegrityIssue");
  // Pre-fix: false (only Array.isArray && length>0 was checked). Post-fix: true.
  assertEqual(
    recordHasIntegrityIssue({ schema_version: 2, errors: "collector boom", service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } }),
    true,
    "a present non-array errors field is an integrity issue"
  );
});

check("Fix 1 serviceBadge malformed-errors healthy-service row is NOT green", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  // schema-2, healthy-looking service, but errors is a string, not an array.
  // Pre-fix renders GREEN (integrity not detected); post-fix caps to amber.
  const latest = { schema_version: 2, errors: "collector boom", service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } };
  const html = serviceBadge(latest);
  assertTrue(!/badge grn/.test(html), "malformed-errors row must NOT render green");
  assertTrue(/badge amb/.test(html), "degraded record renders amber");
});

check("Fix 1 bannerReasons surfaces the malformed errors shape", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // Pre-fix the inline `Array.isArray(latest.errors) && length` push skipped a
  // non-array, so the banner stayed silent. Post-fix the SSOT spread surfaces it.
  const latest = { schema_version: 2, errors: "collector boom", service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /errors field malformed/.test(r)), "banner surfaces the malformed-errors shape");
});

check("Fix 1 guard: empty / absent / non-empty errors behave as before", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const recordHasIntegrityIssue = requireFn(ctx, "recordHasIntegrityIssue");
  const cleanService = { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 };
  // empty array -> clean/green
  assertEqual(recordHasIntegrityIssue({ schema_version: 2, errors: [], service: cleanService }), false, "empty errors stays clean");
  assertTrue(/badge grn/.test(serviceBadge({ schema_version: 2, errors: [], service: cleanService })), "empty errors stays green");
  // absent errors -> clean/green
  assertTrue(/badge grn/.test(serviceBadge({ schema_version: 2, service: cleanService })), "absent errors stays green");
  // non-empty array -> still flagged (unchanged)
  assertEqual(recordHasIntegrityIssue({ schema_version: 2, errors: ["x"], service: cleanService }), true, "non-empty errors still flagged");
  assertTrue(!/badge grn/.test(serviceBadge({ schema_version: 2, errors: ["x"], service: cleanService })), "non-empty errors still capped");
});

// === PR #886 review round (Fix 2: file-level parse errors fail closed) ========
//
// A JSONL file with a malformed line PLUS a valid healthy latest record gives
// parseErrors=1 but, pre-fix, a GREEN badge and a HIDDEN banner — file-level
// parse errors were not part of the dashboard integrity state. Fix 2 threads the
// parse-error count through serviceBadge (caps green) and bannerReasons
// (surfaces the reason). DIFFERENTIAL against the pre-fix html.

check("Fix 2 bannerReasons surfaces file-level parse errors", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // One malformed line + one valid healthy schema-2 record.
  const result = parseJsonl("{bad json}\n{\"schema_version\":2,\"disk\":{\"used_pct\":20},\"service\":{\"active_state\":\"active\",\"sub_state\":\"running\",\"result\":\"success\",\"n_restarts\":0}}");
  assertEqual(result.records.length, 1, "the valid record parses");
  assertEqual(result.errors.length, 1, "the malformed line is a parse error");
  const latest = result.records[result.records.length - 1];
  // Pre-fix: bannerReasons has no parseErrorCount param so this reason is absent.
  const reasons = bannerReasons(latest, result.records, result.errors.length);
  assertTrue(reasons.some(r => /1 line\(s\) failed to parse \(file integrity\)/.test(r)), "parse error surfaces in the banner");
});

check("Fix 2 serviceBadge with parse errors does NOT render green", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const result = parseJsonl("{bad json}\n{\"schema_version\":2,\"disk\":{\"used_pct\":20},\"service\":{\"active_state\":\"active\",\"sub_state\":\"running\",\"result\":\"success\",\"n_restarts\":0}}");
  const latest = result.records[result.records.length - 1];
  // Pre-fix: serviceBadge takes one arg, ignores the file-level signal -> green.
  const html = serviceBadge(latest, result.errors.length > 0);
  assertTrue(!/badge grn/.test(html), "a parse-error file must not render the latest record green");
  assertTrue(/badge amb/.test(html), "file-level integrity caps to amber");
});

check("Fix 2 guard: zero parse errors on a clean record still renders green", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const result = parseJsonl("{\"schema_version\":2,\"disk\":{\"used_pct\":20},\"service\":{\"active_state\":\"active\",\"sub_state\":\"running\",\"result\":\"success\",\"n_restarts\":0}}");
  assertEqual(result.errors.length, 0, "no parse errors on a clean file");
  const latest = result.records[result.records.length - 1];
  assertTrue(/badge grn/.test(serviceBadge(latest, result.errors.length > 0)), "clean file + clean record stays green");
  assertEqual(bannerReasons(latest, result.records, result.errors.length).length, 0, "no banner reason on a clean file");
});

// === PR #886 review round (Fix 3: unavailable cgroup count is not 0) ==========
//
// An OOM-detected record whose cgroup_oom_kills is unavailable (null/absent and
// no usable count) was rendered as `cgroup oom_kill=0`, misrepresenting MISSING
// corroborating evidence as a real zero. Fix 3 renders `?` for an unavailable
// count in the OOM-detected wording. DIFFERENTIAL against the pre-fix html.

check("Fix 3 OOM-detected wording renders '?' for an unavailable cgroup count", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  // oom_killed + result=signal, but no usable cgroup count (null top-level, no
  // nested count). Pre-fix: "cgroup oom_kill=0". Post-fix: "cgroup oom_kill=?".
  const latest = { oom_killed: true, service: { result: "signal" }, cgroup_oom_kills: null };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /cgroup oom_kill=\?/.test(r)), "unavailable count renders as ?");
  assertTrue(!reasons.some(r => /cgroup oom_kill=0\b/.test(r)), "must NOT misreport an unavailable count as 0");
});

check("Fix 3 guard: a real finite cgroup count still renders the number", () => {
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const latest = { oom_killed: true, service: { result: "signal", cgroup_oom_kills: 3 } };
  const reasons = bannerReasons(latest, [latest]);
  assertTrue(reasons.some(r => /cgroup oom_kill=3/.test(r)), "a real count still renders numerically");
  assertTrue(!reasons.some(r => /cgroup oom_kill=\?/.test(r)), "a present count is not masked with ?");
});

// === CLASS fix: malformed health metrics never render false-green ============

check("Fix CLASS shared validation predicates reject coercible malformed values", () => {
  const isStrictFiniteNumber = requireFn(ctx, "isStrictFiniteNumber");
  const isValidUsedPct = requireFn(ctx, "isValidUsedPct");
  const isValidRestartCount = requireFn(ctx, "isValidRestartCount");
  const isMalformedOomKilled = requireFn(ctx, "isMalformedOomKilled");
  assertEqual(isStrictFiniteNumber(40), true, "JSON numbers are accepted");
  assertEqual(isStrictFiniteNumber("40"), false, "coercible strings are rejected");
  assertEqual(isValidUsedPct(40), true, "normal used_pct is valid");
  assertEqual(isValidUsedPct(true), false, "boolean used_pct is malformed");
  assertEqual(isValidUsedPct([80]), false, "array used_pct is malformed");
  assertEqual(isValidRestartCount(null), true, "absent restart count is acceptable");
  assertEqual(isValidRestartCount(0), true, "zero restarts is valid");
  assertEqual(isValidRestartCount(-3), false, "negative restart count is malformed");
  assertEqual(isValidRestartCount("garbage"), false, "non-number restart count is malformed");
  assertEqual(isMalformedOomKilled(true), false, "boolean OOM flag is valid");
  assertEqual(isMalformedOomKilled(null), false, "absent OOM flag is acceptable");
  assertEqual(isMalformedOomKilled("true"), true, "string OOM flag is malformed");
  assertEqual(isMalformedOomKilled({}), true, "object OOM flag is malformed");
});

check("Fix CLASS malformed oom_killed values are amber with a banner reason", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  for (const oom_killed of [1, "true", [true], {}]) {
    const latest = {
      schema_version: 2,
      errors: [],
      oom_killed,
      disk: { used_pct: 20 },
      service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 }
    };
    const html = serviceBadge(latest);
    assertTrue(/badge amb/.test(html), `malformed oom_killed=${JSON.stringify(oom_killed)} must be amber`);
    assertTrue(!/badge grn/.test(html), `malformed oom_killed=${JSON.stringify(oom_killed)} must NOT be green`);
    assertTrue(
      bannerReasons(latest, [latest]).some(r => /oom_killed field malformed/.test(r)),
      `malformed oom_killed=${JSON.stringify(oom_killed)} must raise a banner reason`
    );
  }
  const killed = {
    schema_version: 2,
    errors: [],
    oom_killed: true,
    disk: { used_pct: 20 },
    service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 }
  };
  assertTrue(/badge red/.test(serviceBadge(killed)), "boolean true OOM remains red");
});

check("Fix CLASS malformed disk used_pct values are violet with a banner reason", () => {
  const diskFreePct = requireFn(ctx, "diskFreePct");
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  for (const used_pct of [true, false, [], [80], " "]) {
    const freePct = diskFreePct({ used_pct });
    const latest = {
      schema_version: 2,
      errors: [],
      oom_killed: null,
      disk: { used_pct },
      service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 }
    };
    assertEqual(freePct, null, `malformed used_pct=${JSON.stringify(used_pct)} yields no free pct`);
    assertEqual(diskBadgeClass(freePct), "vio", `malformed used_pct=${JSON.stringify(used_pct)} is violet`);
    assertTrue(
      bannerReasons(latest, [latest]).some(r => /disk metric malformed/.test(r)),
      `malformed used_pct=${JSON.stringify(used_pct)} must raise a banner reason`
    );
  }
  assertEqual(diskFreePct({ used_pct: 40 }), 60, "normal used_pct still maps to free pct");
  assertEqual(diskBadgeClass(diskFreePct({ used_pct: 40 })), "grn", "normal used_pct stays green");
  assertEqual(diskFreePct({ used_pct: null }), null, "null used_pct remains unavailable");
});

check("Fix CLASS malformed restart counts are amber with a banner reason", () => {
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const negative = {
    schema_version: 2,
    errors: [],
    oom_killed: null,
    disk: { used_pct: 20 },
    service: { active_state: "active", sub_state: "running", result: "success", n_restarts: -3 }
  };
  const negativeHtml = serviceBadge(negative);
  assertTrue(/badge amb/.test(negativeHtml), "negative restart count must be amber");
  assertTrue(!/badge grn/.test(negativeHtml), "negative restart count must NOT be green");
  assertTrue(
    bannerReasons(negative, [negative]).some(r => /service restart count malformed/.test(r)),
    "negative restart count must raise a malformed-restart banner reason"
  );

  const zero = { ...negative, service: { ...negative.service, n_restarts: 0 } };
  assertTrue(/badge grn/.test(serviceBadge(zero)), "zero restart count remains green-eligible");

  const positive = { ...negative, service: { ...negative.service, n_restarts: 2 } };
  assertTrue(/badge amb/.test(serviceBadge(positive)), "positive restart count remains amber");
  assertTrue(
    bannerReasons(positive, [positive]).some(r => /auto-restarted 2/.test(r)),
    "positive restart count keeps the existing auto-restart reason"
  );

  const nonNumeric = { ...negative, service: { ...negative.service, n_restarts: "garbage" } };
  assertTrue(/badge amb/.test(serviceBadge(nonNumeric)), "non-numeric restart count remains amber");
});

// F5/F6/F7 (#886 relay round): the chart series, the cgroup count, and the
// restart trend must route through the SAME shared strict predicates the
// badge/banner use -- never bare Number() coercion -- so a malformed-but-coercible
// value can never produce a clean or self-contradictory signal. Each assertion
// goes RED against the pre-fix HEAD copy (run with the html path argument).

check("F5 malformed disk used_pct is not plotted as a clean point", () => {
  const drawTimeSeries = requireFn(ctx, "drawTimeSeries");
  const diskBadgeClass = requireFn(ctx, "diskBadgeClass");
  const diskFreePct = requireFn(ctx, "diskFreePct");
  const records = [{ disk: { used_pct: "0" }, sampled_at: "t0" }];
  const container = ctx.document.createElement("div");
  drawTimeSeries(container, {
    records,
    unit: "percent",
    yMin: 0,
    yMax: 100,
    formatter: value => String(value),
    // No explicit `valid`: the DEFAULT must already reject a coercible string,
    // proving the Number(raw) coercion is gone.
    series: [{ name: "Disk used", value: record => record.disk && record.disk.used_pct }]
  });
  const renderedEmpty = container.children.some(child => child.className === "empty");
  assertTrue(renderedEmpty, 'chart must render the empty placeholder for used_pct="0", not a finite point');
  // ...and the badge agrees it is malformed (violet) -- chart and badge no longer contradict.
  assertEqual(diskBadgeClass(diskFreePct({ used_pct: "0" })), "vio", 'badge must flag used_pct="0" malformed');
});

check("F6 malformed cgroup_oom_kills reads as unavailable, not a fabricated count", () => {
  const cgroupOomCount = requireFn(ctx, "cgroupOomCount");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  assertTrue(Number.isNaN(cgroupOomCount({ cgroup_oom_kills: "0" })), '"0" string must be NaN (unavailable)');
  assertTrue(Number.isNaN(cgroupOomCount({ cgroup_oom_kills: "0x10" })), '"0x10" must be NaN');
  assertTrue(Number.isNaN(cgroupOomCount({ cgroup_oom_kills: "1e3" })), '"1e3" must be NaN');
  assertEqual(cgroupOomCount({ cgroup_oom_kills: 3 }), 3, "a genuine integer count is preserved");
  const record = {
    schema_version: 2,
    oom_killed: true,
    cgroup_oom_kills: "0",
    service: { active_state: "active", sub_state: "running", result: "signal", n_restarts: 0 }
  };
  const oomLine = bannerReasons(record, [record]).find(reason => reason.includes("OOM detected"));
  assertTrue(
    oomLine !== undefined && oomLine.includes("oom_kill=?"),
    `malformed cgroup count must render as "?" in the banner, got: ${oomLine}`
  );
});

check("F7 restart trend does not fire on a malformed n_restarts the badge rejects", () => {
  const restartIncreased = requireFn(ctx, "restartIncreased");
  const isValidRestartCount = requireFn(ctx, "isValidRestartCount");
  assertEqual(isValidRestartCount("0"), false, 'badge predicate rejects the string "0"');
  const malformedHistory = [
    { service: { active_state: "active", sub_state: "running", result: "success", n_restarts: "0" } },
    { service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 1 } }
  ];
  assertEqual(restartIncreased(malformedHistory), false, 'trend must not fire on a malformed historical "0"');
  const realIncrease = [
    { service: { n_restarts: 0 } },
    { service: { n_restarts: 2 } }
  ];
  assertEqual(restartIncreased(realIncrease), true, "a real numeric increase still fires");
});

// Class-closure for the viewer (#886 coverage audit): every numeric health field
// is read through ONE validated accessor (diskUsedPct / restartCount / rssBytes /
// memAvailableBytes / cgroupOomCount), so the card, badge, banner, and chart can
// never disagree; a CI fence bans raw coercion so a new consumer cannot re-introduce
// the leak; and a field x hostile-type matrix asserts the invariant exhaustively.

check("C2-G1 disk:0 corrupt scalar -> chart and card both reject (no contradiction)", () => {
  const diskUsedPct = requireFn(ctx, "diskUsedPct");
  const diskFreePct = requireFn(ctx, "diskFreePct");
  const drawTimeSeries = requireFn(ctx, "drawTimeSeries");
  const rec = { schema_version: 2, sampled_at: "t0", disk: 0, service: { active_state: "active", sub_state: "running", n_restarts: 0 } };
  assertEqual(diskUsedPct(rec), null, "diskUsedPct must reject a falsy-numeric disk parent (disk:0)");
  assertEqual(diskFreePct(rec.disk), null, "disk card already rejects disk:0");
  const container = ctx.document.createElement("div");
  drawTimeSeries(container, { records: [rec], unit: "percent", yMin: 0, yMax: 100, formatter: value => String(value), series: [{ name: "Disk used", value: record => diskUsedPct(record) }] });
  assertTrue(container.children.some(child => child.className === "empty"), "disk chart must not plot a clean 0%-used point for disk:0");
});

check("C2-G2 coercible n_restarts -> restart card EMPTY (no contradiction with malformed badge)", () => {
  const restartCount = requireFn(ctx, "restartCount");
  const formatNumber = requireFn(ctx, "formatNumber");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const rec = { schema_version: 2, sampled_at: "t0", service: { active_state: "active", sub_state: "running", result: "success", n_restarts: "5" } };
  assertEqual(restartCount(rec), null, "restartCount must reject a coercible string n_restarts");
  assertEqual(formatNumber(restartCount(rec)), formatNumber(null), "restart card must be EMPTY for a malformed count, not a fabricated number");
  assertTrue(/badge amb/.test(serviceBadge(rec)), "badge stays amber for malformed restart count");
});

check("FENCE: no raw Number()/parseInt/parseFloat coercion in the viewer", () => {
  const html = readFileSync(htmlPath, "utf8");
  const script = extractInlineScript(html)
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\/\/[^\n]*/g, "");
  const banned = script.match(/\b(?:Number|parseInt|parseFloat)\s*\(/g) || [];
  assertEqual(banned.length, 0, `numeric values must come from validated field accessors, not coercion; found: ${banned.join(", ")}`);
});

check("MATRIX: numeric health fields x hostile types -> accessor null, chart==accessor, no false-green, no crash", () => {
  const diskUsedPct = requireFn(ctx, "diskUsedPct");
  const restartCount = requireFn(ctx, "restartCount");
  const rssBytes = requireFn(ctx, "rssBytes");
  const memAvailableBytes = requireFn(ctx, "memAvailableBytes");
  const serviceBadge = requireFn(ctx, "serviceBadge");
  const bannerReasons = requireFn(ctx, "bannerReasons");
  const drawTimeSeries = requireFn(ctx, "drawTimeSeries");

  const HOSTILE = ["0", "5", "", " ", "0x10", "1e3", "-1", [], [5], {}, true, false, -1, NaN, Infinity, -Infinity, null, undefined];

  const chartPlots = (accessorFn, record) => {
    const container = ctx.document.createElement("div");
    drawTimeSeries(container, { records: [record], unit: "x", yMin: 0, yMax: 100, formatter: value => String(value), series: [{ name: "s", value: r => accessorFn(r) }] });
    return !container.children.some(child => child.className === "empty");
  };

  for (const h of HOSTILE) {
    for (const disk of [h, { used_pct: h }]) {
      const rec = { schema_version: 2, sampled_at: "t", disk };
      assertEqual(diskUsedPct(rec), null, `diskUsedPct must reject hostile disk=${JSON.stringify(disk)}`);
      assertEqual(chartPlots(diskUsedPct, rec), false, `disk chart must not plot for hostile disk=${JSON.stringify(disk)}`);
    }
    assertEqual(restartCount({ service: { n_restarts: h } }), null, `restartCount hostile ${JSON.stringify(h)}`);
    assertEqual(rssBytes({ process: { rss_bytes: h } }), null, `rssBytes hostile ${JSON.stringify(h)}`);
    assertEqual(memAvailableBytes({ memory: { mem_available_bytes: h } }), null, `memAvailableBytes hostile ${JSON.stringify(h)}`);
    if (h !== null && h !== undefined) {
      const rec = { schema_version: 2, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: h } };
      assertTrue(!/badge grn/.test(serviceBadge(rec)), `present malformed n_restarts must not be green: ${JSON.stringify(h)}`);
    }
    bannerReasons({ schema_version: 2, disk: h, service: { active_state: "active", sub_state: "running", n_restarts: h }, cgroup_oom_kills: h, oom_killed: h }, []);
  }

  // positive control: valid values ARE accepted, plotted, and green-eligible.
  assertEqual(diskUsedPct({ disk: { used_pct: 42 } }), 42, "valid used_pct accepted");
  assertEqual(chartPlots(diskUsedPct, { disk: { used_pct: 42 } }), true, "valid used_pct plots");
  assertEqual(rssBytes({ process: { rss_bytes: 4096 } }), 4096, "valid rss accepted");
  assertEqual(restartCount({ service: { n_restarts: 0 } }), 0, "valid restart 0 accepted");
  assertTrue(/badge grn/.test(serviceBadge({ schema_version: 2, service: { active_state: "active", sub_state: "running", result: "success", n_restarts: 0 } })), "clean record is green-eligible");
});

// --- report ------------------------------------------------------------------
console.log(`\n${passed} passed, ${failures.length} failed`);
if (failures.length > 0) {
  process.exit(1);
}
