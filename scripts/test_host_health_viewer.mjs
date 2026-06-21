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
check("D6 parseJsonl still accepts real objects", () => {
  const parseJsonl = requireFn(ctx, "parseJsonl");
  const result = parseJsonl("{\"a\":1}\n{\"b\":2}");
  assertEqual(result.records.length, 2);
  assertEqual(result.errors.length, 0);
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
  const latest = { oom_killed: null, service: { active_state: "active", n_restarts: 0, result: "success" } };
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
  const html = serviceBadge({ oom_killed: null, service: { active_state: "active", sub_state: "running", n_restarts: 0, result: "success" } });
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

// --- report ------------------------------------------------------------------
console.log(`\n${passed} passed, ${failures.length} failed`);
if (failures.length > 0) {
  process.exit(1);
}
