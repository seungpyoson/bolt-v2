#!/usr/bin/env python3
"""Relocated CI workflow hygiene analyzer tests."""

from __future__ import annotations

import sys

from ci_workflow_hygiene_test_helpers import (
    GATE_NAME,
    GATE_NEEDS,
    assert_no_inline_matrix_key,
    ci_provenance_config_fixture,
    inline_matrix_values,
    load_verifier,
    replace_once,
    repo_workflow_text,
    runner_config_load_error,
    shard_partition_argument_denominators,
    without_inline_need,
)
from workflow_expression_analysis import one_indexed_sequence

def assert_merge_group_support_gaps_are_reported() -> None:
    # Non-vacuous mutation tests for the merge queue (merge_group) lane:
    # the real workflows/config must be clean, and each mutation must surface
    # its own specific error. A skipped required check counts as passing in
    # GitHub, so every gap here would silently let an unvalidated commit merge.
    verifier = load_verifier()
    ci_workflow = repo_workflow_text(".github/workflows/ci.yml")
    actionlint_workflow = repo_workflow_text(".github/workflows/actionlint.yml")
    backtester_workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")
    tracer_group_arm = (
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && inputs.ra001a_durable_tracer == true\n"
        "        && format('bvs-ra001a-tracer-{0}', github.sha)\n"
    )
    if tracer_group_arm not in backtester_workflow:
        raise AssertionError(
            "RA-001a tracer dispatches must have an exact-head source-SHA concurrency arm"
        )

    # Baseline: real workflows declare merge_group and resolve clean.
    if verifier.verify_workflow(ci_workflow):
        raise AssertionError(
            f"real ci.yml must be merge_group-clean, got: {verifier.verify_workflow(ci_workflow)}"
        )
    actionlint_baseline = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_workflow}
    )
    if any("merge_group" in error for error in actionlint_baseline):
        raise AssertionError(
            f"real actionlint.yml must be merge_group-clean, got: {actionlint_baseline}"
        )
    backtester_baseline = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_workflow}
    )
    if any("merge_group" in error for error in backtester_baseline):
        raise AssertionError(
            f"real backtester-ci.yml must be merge_group-clean, got: {backtester_baseline}"
        )

    tracer_string_input = replace_once(
        backtester_workflow,
        '      ra001a_durable_tracer:\n'
        '        description: "Run the non-required RA-001a registry-complete durable tracer"\n'
        "        type: boolean\n",
        '      ra001a_durable_tracer:\n'
        '        description: "Run the non-required RA-001a registry-complete durable tracer"\n'
        "        type: string\n",
    )
    tracer_string_input_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": tracer_string_input}
    )
    if not any(
        "workflow_dispatch input ra001a_durable_tracer must be boolean" in error
        for error in tracer_string_input_errors
    ):
        raise AssertionError("backtester governance must reject an untyped tracer dispatch input")

    for input_name in ("issue_789", "ra001a_durable_tracer"):
        input_header = f"      {input_name}:\n"
        input_start = backtester_workflow.index(input_header)
        default_start = backtester_workflow.index("        default: false\n", input_start)
        default_true_workflow = (
            backtester_workflow[:default_start]
            + "        default: true\n"
            + backtester_workflow[default_start + len("        default: false\n"):]
        )
        default_true_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": default_true_workflow}
        )
        if not any(
            f"workflow_dispatch input {input_name} must default false" in error
            for error in default_true_errors
        ):
            raise AssertionError(
                f"backtester governance must reject default-enabled {input_name} dispatch"
            )

    dispatch_uses_head_policy = replace_once(
        backtester_workflow,
        "if: github.event_name == 'pull_request' || github.event_name == 'merge_group' || github.event_name == 'workflow_dispatch'",
        "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
    )
    dispatch_uses_head_policy_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": dispatch_uses_head_policy}
    )
    if not any(
        "Prepare trusted base policy tree" in error
        or "workflow_dispatch" in error and "ci-policy job must include" in error
        for error in dispatch_uses_head_policy_errors
    ):
        raise AssertionError("backtester governance must reject head-authorized dispatch policy")

    inline_policy_parser = replace_once(
        backtester_workflow,
        "          tracer_args=()\n",
        "          tracer_args=()\n"
        "          python3 -c 'import tomllib; tomllib.loads(\"\")'\n",
    )
    inline_policy_parser_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": inline_policy_parser}
    )
    if not any(
        "typed trusted resolver instead of an inline policy parser" in error
        for error in inline_policy_parser_errors
    ):
        raise AssertionError("backtester governance must reject an alternate inline policy parser")

    interpolated_dispatch_input = replace_once(
        backtester_workflow,
        '&& "$RA001A_DURABLE_TRACER_REQUESTED" == "true" ]]; then',
        '&& "${{ github.event.inputs.ra001a_durable_tracer }}" == "true" ]]; then',
    )
    interpolated_dispatch_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": interpolated_dispatch_input}
    )
    if not any(
        "typed inputs context and env-mediated shell access" in error
        for error in interpolated_dispatch_errors
    ):
        raise AssertionError("backtester governance must reject shell-interpolated dispatch inputs")

    backtester_shared_tracer_group = replace_once(
        backtester_workflow,
        tracer_group_arm,
        "",
    )
    shared_tracer_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_shared_tracer_group}
    )
    if not any(
        "RA-001a tracer dispatches must use exact-head source-SHA concurrency" in error
        for error in shared_tracer_errors
    ):
        raise AssertionError(
            "backtester concurrency governance must reject tracer dispatches sharing the ordinary dispatch group"
        )

    backtester_unbounded_tracer_group = replace_once(
        backtester_workflow,
        "        && format('bvs-ra001a-tracer-{0}', github.sha)\n",
        "        && format('bvs-ra001a-tracer-{0}', github.run_id)\n",
    )
    unbounded_tracer_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_unbounded_tracer_group}
    )
    if not any(
        "RA-001a tracer dispatches must use exact-head source-SHA concurrency" in error
        for error in unbounded_tracer_errors
    ):
        raise AssertionError(
            "backtester concurrency governance must reject an unbounded per-run tracer group"
        )

    backtester_cancels_tracer_dispatch = replace_once(
        backtester_workflow,
        "        || (github.event_name == 'workflow_dispatch'\n"
        "            && inputs.ra001a_durable_tracer != true) }}",
        "        || github.event_name == 'workflow_dispatch' }}",
    )
    cancels_tracer_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_cancels_tracer_dispatch}
    )
    if not any(
        "RA-001a tracer dispatches must not be cancelled" in error
        for error in cancels_tracer_errors
    ):
        raise AssertionError(
            "backtester cancellation governance must reject the older all-dispatch cancellation form"
        )

    tracer_policy_output = (
        "      ra001a_durable_tracer_required: "
        "${{ steps.policy.outputs.ra001a_durable_tracer_required }}\n"
    )
    archive_timeout_policy_output = (
        "      backtester_test_archive_timeout_minutes: "
        "${{ steps.policy.outputs.backtester_test_archive_timeout_minutes }}\n"
    )
    issue_789_timeout_policy_output = (
        "      backtester_issue_789_timeout_minutes: "
        "${{ steps.policy.outputs.backtester_issue_789_timeout_minutes }}\n"
    )
    tracer_policy_probe = (
        "          tracer_args=()\n"
        "          if python3 \"$policy_script\" ci-policy --help | grep -q -- \"--ra001a-durable-tracer-requested\"; then\n"
        "            tracer_args=(--ra001a-durable-tracer-requested \"$RA001A_DURABLE_TRACER_REQUESTED\")\n"
        "          elif [[ \"$RA001A_DURABLE_TRACER_REQUESTED\" == \"true\" ]]; then\n"
        "            echo \"trusted policy resolver does not support the requested RA-001a tracer\" >&2\n"
        "            exit 1\n"
        "          fi\n"
    )
    tracer_archive_guard = (
        " || needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true'"
    )
    for label, fragment in (
        ("policy output", tracer_policy_output),
        ("trusted archive timeout output", archive_timeout_policy_output),
        ("trusted issue #789 timeout output", issue_789_timeout_policy_output),
        ("trusted-base compatibility probe", tracer_policy_probe),
        ("test-archive reachability guard", tracer_archive_guard),
    ):
        if fragment not in backtester_workflow:
            raise AssertionError(f"RA-001a tracer {label} is missing from the real workflow")

    unprobed_policy_workflow = replace_once(
        backtester_workflow,
        tracer_policy_probe,
        "          tracer_args=(--ra001a-durable-tracer-requested \"$RA001A_DURABLE_TRACER_REQUESTED\")\n",
    )
    unprobed_policy_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unprobed_policy_workflow}
    )
    if not any("ci-policy job must include tracer_args=()" in error for error in unprobed_policy_errors):
        raise AssertionError("backtester policy must probe trusted-base support for the tracer argument")

    unreachable_tracer_workflow = replace_once(
        backtester_workflow,
        tracer_archive_guard,
        "",
    )
    unreachable_tracer_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unreachable_tracer_workflow}
    )
    if not any(
        "RA-001a tracer dispatch must reach test-archive through governed policy output" in error
        for error in unreachable_tracer_errors
    ):
        raise AssertionError(
            "backtester governance must reject a tracer request that cannot reach its archive job"
        )

    tracer_test_skip = (
        "      - name: test\n"
        "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required != 'true' }}\n"
    )
    tracer_timeout = (
        '          timeout --signal=TERM --kill-after="${RA001A_TERMINATION_GRACE_SECONDS}s" '
        '"${RA001A_MAX_WALL_SECONDS}s" \\\n'
        "            just bte-test-archive-run \\\n"
    )
    tracer_byte_env = (
        "          BOLT_RA001A_MAX_TOTAL_SELECTED_OBJECT_BYTES: "
        "${{ needs.ci-policy.outputs.ra001a_max_total_selected_object_bytes }}\n"
    )
    tracer_checkout_policy_env = (
        "          BOLT_RA001A_ALLOWED_IGNORED_RUNTIME_ROOTS: "
        "${{ needs.ci-policy.outputs.ra001a_allowed_ignored_runtime_roots }}\n"
        "          BOLT_RA001A_MAX_IGNORED_ENTRY_BYTES: "
        "${{ needs.ci-policy.outputs.ra001a_max_ignored_entry_bytes }}\n"
        "          BOLT_RA001A_MAX_IGNORED_ENTRIES: "
        "${{ needs.ci-policy.outputs.ra001a_max_ignored_entries }}\n"
    )
    tracer_no_tests_fail = (
        "            --run-ignored ignored-only \\\n"
        "            --no-tests=fail \\\n"
    )
    tracer_evidence_requirements = (
        (
            "AWS policy gate",
            "      - name: Configure AWS credentials for RA-001a durable tracer\n        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n",
            "      - name: Configure AWS credentials for RA-001a durable tracer\n",
            "RA-001a tracer AWS credentials must remain role-scoped and policy-gated",
        ),
        (
            "AWS action pin",
            "      - name: Configure AWS credentials for RA-001a durable tracer\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n"
            "        uses: aws-actions/configure-aws-credentials@e7f100cf4c008499ea8adda475de1042d6975c7b # v6.2.0\n",
            "      - name: Configure AWS credentials for RA-001a durable tracer\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n"
            "        uses: aws-actions/configure-aws-credentials@e7f100cf4c008499ea8adda475de1042d6975c7b-suffix\n",
            "RA-001a tracer AWS action must use exactly the reviewed 40-hex commit",
        ),
        (
            "AWS role binding",
            "          role-to-assume: ${{ vars.AWS_RA001A_TRACER_ROLE_ARN }}\n",
            "          role-to-assume: arn:aws:iam::000000000000:role/untrusted\n",
            "RA-001a tracer AWS credentials must remain role-scoped and policy-gated",
        ),
        (
            "AWS region binding",
            "          aws-region: ${{ vars.AWS_RA001A_TRACER_REGION }}\n",
            "          aws-region: us-east-1\n",
            "RA-001a tracer AWS credentials must remain role-scoped and policy-gated",
        ),
        (
            "worker executable check",
            '          test -x "$WORKER_EXECUTABLE" || { echo "RA-001a worker sidecar is missing or not executable"; exit 1; }\n',
            '          true || test -x "$WORKER_EXECUTABLE" || { echo "RA-001a worker sidecar is missing or not executable"; exit 1; }\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "receipt directory creation",
            '          mkdir -p "$(dirname "$RECEIPT_PATH")"\n',
            "",
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "fresh receipt path check",
            '          test ! -e "$RECEIPT_PATH" || { echo "RA-001a receipt path already exists"; exit 1; }\n',
            '          true || test ! -e "$RECEIPT_PATH" || { echo "RA-001a receipt path already exists"; exit 1; }\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "duplicated worker digest",
            '          worker_sha256="$(sha256sum "$WORKER_EXECUTABLE" | cut -d \' \' -f 1)"\n',
            '          worker_sha256="$(sha256sum "$WORKER_EXECUTABLE" | cut -d \' \' -f 1)"\n'
            '          worker_sha256="$(sha256sum "$WORKER_EXECUTABLE" | cut -d \' \' -f 1)"\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "worker digest validation",
            '          [[ "$worker_sha256" =~ ^[0-9a-f]{64}$ ]] || { echo "RA-001a worker SHA-256 is invalid"; exit 1; }\n',
            '          true || [[ "$worker_sha256" =~ ^[0-9a-f]{64}$ ]] || { echo "RA-001a worker SHA-256 is invalid"; exit 1; }\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "duplicated worker digest output",
            '          echo "worker_sha256=$worker_sha256" >> "$GITHUB_OUTPUT"\n',
            '          echo "worker_sha256=$worker_sha256" >> "$GITHUB_OUTPUT"\n'
            '          echo "worker_sha256=$worker_sha256" >> "$GITHUB_OUTPUT"\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "duplicated receipt path output",
            '          echo "receipt_path=$RECEIPT_PATH" >> "$GITHUB_OUTPUT"\n',
            '          echo "receipt_path=$RECEIPT_PATH" >> "$GITHUB_OUTPUT"\n'
            '          echo "receipt_path=$RECEIPT_PATH" >> "$GITHUB_OUTPUT"\n',
            "RA-001a tracer inputs must execute the worker and receipt binding chain exactly once in order",
        ),
        (
            "source revision binding",
            "          BOLT_RA001A_SOURCE_REVISION: ${{ github.sha }}\n",
            "          BOLT_RA001A_SOURCE_REVISION: refs/heads/main\n",
            "RA-001a tracer must bind exact-head evidence and require a non-empty receipt",
        ),
        (
            "worker digest binding",
            "          BOLT_RA001A_WORKER_SHA256: ${{ steps.ra001a-durable-tracer-inputs.outputs.worker_sha256 }}\n",
            "",
            "RA-001a tracer must bind exact-head evidence and require a non-empty receipt",
        ),
        (
            "receipt binding",
            "          BOLT_RA001A_RECEIPT_PATH: ${{ steps.ra001a-durable-tracer-inputs.outputs.receipt_path }}\n",
            "",
            "RA-001a tracer must bind exact-head evidence and require a non-empty receipt",
        ),
        (
            "exact tracer selector",
            "            -E 'binary(=backtesting_vertical_slice_tests) & test(=backtesting_vertical_slice_source_universe_durable_tracer::registry_complete_ra001a_live_tracer_runs_every_committed_pack)'\n",
            "            -E 'binary(=backtesting_vertical_slice_tests)'\n",
            "RA-001a tracer must bind exact-head evidence and require a non-empty receipt",
        ),
        (
            "non-empty receipt check",
            '          test -s "$BOLT_RA001A_RECEIPT_PATH" || { echo "RA-001a receipt is missing or empty"; exit 1; }\n',
            '          true || test -s "$BOLT_RA001A_RECEIPT_PATH" || { echo "RA-001a receipt is missing or empty"; exit 1; }\n',
            "RA-001a tracer must execute its exact selector before exactly one live non-empty receipt guard",
        ),
        (
            "success-gated receipt upload",
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' && steps.ra001a-durable-tracer.outcome == 'success' }}\n",
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n",
            "RA-001a receipt upload must require successful evidence production and fail on absence",
        ),
        (
            "receipt upload action pin",
            "      - name: Upload RA-001a durable tracer receipt\n"
            "        id: upload-ra001a-durable-tracer-receipt\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' && steps.ra001a-durable-tracer.outcome == 'success' }}\n"
            "        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1\n",
            "      - name: Upload RA-001a durable tracer receipt\n"
            "        id: upload-ra001a-durable-tracer-receipt\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' && steps.ra001a-durable-tracer.outcome == 'success' }}\n"
            "        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a-suffix\n",
            "RA-001a receipt upload action must use exactly the reviewed 40-hex commit",
        ),
        (
            "receipt upload path",
            "          path: ${{ steps.ra001a-durable-tracer-inputs.outputs.receipt_path }}\n",
            "          path: ${{ runner.temp }}\n",
            "RA-001a receipt upload must require successful evidence production and fail on absence",
        ),
        (
            "missing-receipt upload failure",
            "          path: ${{ steps.ra001a-durable-tracer-inputs.outputs.receipt_path }}\n          if-no-files-found: error\n",
            "          path: ${{ steps.ra001a-durable-tracer-inputs.outputs.receipt_path }}\n          if-no-files-found: warn\n",
            "RA-001a receipt upload must require successful evidence production and fail on absence",
        ),
        (
            "trusted policy script presence check",
            '          test -n "$policy_script" || { echo "trusted base policy script is required for $EVENT_NAME" >&2; exit 1; }\n',
            '          true || test -n "$policy_script" || { echo "trusted base policy script is required for $EVENT_NAME" >&2; exit 1; }\n',
            "ci-policy job must execute each trusted policy path guard exactly once after binding and before use",
        ),
        (
            "trusted policy config presence check",
            '          test -n "$policy_config" || { echo "trusted base policy config is required for $EVENT_NAME" >&2; exit 1; }\n',
            '          true || test -n "$policy_config" || { echo "trusted base policy config is required for $EVENT_NAME" >&2; exit 1; }\n',
            "ci-policy job must execute each trusted policy path guard exactly once after binding and before use",
        ),
    )
    archive_job_timeout = (
        "    timeout-minutes: "
        "${{ fromJSON(needs.ci-policy.outputs.backtester_test_archive_timeout_minutes) }}\n"
    )
    issue_789_job_timeout = (
        "    timeout-minutes: "
        "${{ fromJSON(needs.ci-policy.outputs.backtester_issue_789_timeout_minutes) }}\n"
    )
    timeout_output_capture = (
        '          policy_output="$RUNNER_TEMP/backtester-ci-policy-output"\n'
        '          test ! -e "$policy_output" || { echo "trusted policy output path already exists" >&2; exit 1; }\n'
    )
    timeout_output_validation = (
        "          for positive_policy_key in \\\n"
        "            backtester_test_archive_timeout_minutes \\\n"
        "            backtester_issue_789_timeout_minutes \\\n"
        "            ra001a_max_registry_packs \\\n"
        "            ra001a_max_total_selected_object_bytes \\\n"
        "            ra001a_max_wall_seconds \\\n"
        "            ra001a_termination_grace_seconds \\\n"
        "            ra001a_max_ignored_entry_bytes \\\n"
        "            ra001a_max_ignored_entries; do\n"
        "            value_counts=\"$(awk -F= -v key=\"$positive_policy_key\" '$1 == key { all += 1; if ($2 ~ /^[1-9][0-9]*$/) valid += 1 } END { printf \"%d:%d\", all, valid }' \"$policy_output\")\"\n"
        "            [[ \"$value_counts\" == \"1:1\" ]] || {\n"
        "              echo \"trusted policy resolver must emit exactly one positive ${positive_policy_key}\" >&2\n"
        "              exit 1\n"
        "            }\n"
        "          done\n"
        "          ignored_roots_counts=\"$(awk -F= '$1 == \"ra001a_allowed_ignored_runtime_roots\" { all += 1; if ($2 ~ /^[-A-Za-z0-9._\\/]+(,[-A-Za-z0-9._\\/]+)*$/) valid += 1 } END { printf \"%d:%d\", all, valid }' \"$policy_output\")\"\n"
        "          [[ \"$ignored_roots_counts\" == \"1:1\" ]] || {\n"
        "            echo \"trusted policy resolver must emit exactly one normalized ignored-runtime root list\" >&2\n"
        "            exit 1\n"
        "          }\n"
        "          tracer_required_counts=\"$(awk -F= '$1 == \"ra001a_durable_tracer_required\" { all += 1; if ($2 == \"true\" || $2 == \"false\") valid += 1 } END { printf \"%d:%d\", all, valid }' \"$policy_output\")\"\n"
        "          [[ \"$tracer_required_counts\" == \"1:1\" ]] || {\n"
        "            echo \"trusted policy resolver must emit exactly one boolean ra001a_durable_tracer_required\" >&2\n"
        "            exit 1\n"
        "          }\n"
        "          tracer_required=\"$(awk -F= '$1 == \"ra001a_durable_tracer_required\" { print $2 }' \"$policy_output\")\"\n"
        "          [[ \"$tracer_required\" == \"$RA001A_DURABLE_TRACER_REQUESTED\" ]] || {\n"
        "            echo \"trusted policy resolver tracer decision does not match the dispatch request\" >&2\n"
        "            exit 1\n"
        "          }\n"
        "          cat \"$policy_output\" >> \"$GITHUB_OUTPUT\"\n"
    )
    for label, fragment in (
        ("partition-suite skip", tracer_test_skip),
        ("aggregate wall timeout", tracer_timeout),
        ("aggregate source-byte limit", tracer_byte_env),
        ("checkout cleanliness policy", tracer_checkout_policy_env),
        ("empty-selection failure", tracer_no_tests_fail),
        ("whole archive job timeout", archive_job_timeout),
        ("whole issue #789 job timeout", issue_789_job_timeout),
        ("trusted timeout output capture", timeout_output_capture),
        ("trusted timeout output validation", timeout_output_validation),
    ):
        if fragment not in backtester_workflow:
            raise AssertionError(f"RA-001a tracer {label} is missing from the real workflow")

    for label, fragment, replacement, expected in tracer_evidence_requirements:
        if fragment not in backtester_workflow:
            raise AssertionError(f"RA-001a tracer {label} is missing from the real workflow")
        mutated = replace_once(backtester_workflow, fragment, replacement)
        mutation_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": mutated}
        )
        if not any(expected in error for error in mutation_errors):
            raise AssertionError(
                f"RA-001a tracer {label} mutation did not fail with {expected!r}: "
                f"{mutation_errors}"
            )

    nested_policy_guards = replace_once(
        backtester_workflow,
        '          test -n "$policy_script" || { echo "trusted base policy script is required for $EVENT_NAME" >&2; exit 1; }\n'
        '          test -n "$policy_config" || { echo "trusted base policy config is required for $EVENT_NAME" >&2; exit 1; }\n',
        '          if false; then\n'
        '            test -n "$policy_script" || { echo "trusted base policy script is required for $EVENT_NAME" >&2; exit 1; }\n'
        '            test -n "$policy_config" || { echo "trusted base policy config is required for $EVENT_NAME" >&2; exit 1; }\n'
        '          fi\n',
    )
    nested_policy_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": nested_policy_guards}
    )
    if not any(
        "ci-policy job must execute each trusted policy path guard exactly once after binding and before use"
        in error
        for error in nested_policy_errors
    ):
        raise AssertionError(
            "backtester governance must reject trusted policy guards nested in dead control flow"
        )

    nested_policy_use = replace_once(
        backtester_workflow,
        '          python3 "$policy_script" ci-policy \\\n',
        '          if false; then\n'
        '            python3 "$policy_script" ci-policy \\\n',
    )
    nested_policy_use = replace_once(
        nested_policy_use,
        '            | tee "$policy_output"\n',
        '            | tee "$policy_output"\n'
        '          fi\n',
    )
    nested_policy_use_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": nested_policy_use}
    )
    if not any(
        "ci-policy job must execute each trusted policy path guard exactly once after binding and before use"
        in error
        for error in nested_policy_use_errors
    ):
        raise AssertionError(
            "backtester governance must reject a trusted policy invocation nested in dead control flow"
        )

    continue_on_error_mutations = (
        (
            "test-archive job",
            archive_job_timeout,
            archive_job_timeout + "    continue-on-error: true\n",
            "RA-001a test-archive job must fail closed without job-level continue-on-error",
        ),
        (
            "tracer producer step",
            "        id: ra001a-durable-tracer\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n",
            "        id: ra001a-durable-tracer\n"
            "        continue-on-error: true\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' }}\n",
            "Run RA-001a registry-complete durable tracer must fail closed without continue-on-error",
        ),
        (
            "receipt upload step",
            "        id: upload-ra001a-durable-tracer-receipt\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' && steps.ra001a-durable-tracer.outcome == 'success' }}\n",
            "        id: upload-ra001a-durable-tracer-receipt\n"
            "        continue-on-error: true\n"
            "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required == 'true' && steps.ra001a-durable-tracer.outcome == 'success' }}\n",
            "Upload RA-001a durable tracer receipt must fail closed without continue-on-error",
        ),
    )
    for label, fragment, replacement, expected in continue_on_error_mutations:
        mutated = replace_once(backtester_workflow, fragment, replacement)
        mutation_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": mutated}
        )
        if not any(expected in error for error in mutation_errors):
            raise AssertionError(
                f"RA-001a {label} continue-on-error mutation did not fail with "
                f"{expected!r}: {mutation_errors}"
            )

    tracer_dual_request_guard = (
        '          if [[ "$EVENT_NAME" == "workflow_dispatch" \\\n'
        '            && "$ISSUE_789_REQUESTED" == "true" \\\n'
        '            && "$RA001A_DURABLE_TRACER_REQUESTED" == "true" ]]; then\n'
        '            echo "issue #789 and the RA-001a durable tracer are mutually exclusive managed-heavy requests" >&2\n'
        "            exit 1\n"
        "          fi\n"
    )
    tracer_role_preflight = (
        '            test -n "$RA001A_TRACER_ROLE_ARN" || { echo "AWS_RA001A_TRACER_ROLE_ARN is required" >&2; exit 1; }\n'
    )
    tracer_region_preflight = (
        '            test -n "$RA001A_TRACER_REGION" || { echo "AWS_RA001A_TRACER_REGION is required" >&2; exit 1; }\n'
    )
    minio_skip = (
        "      - name: Setup BVS MinIO S3 smoke\n"
        "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required != 'true' }}\n"
    )
    minio_test_skip = (
        "      - name: test BVS MinIO S3 smoke\n"
        "        if: ${{ needs.ci-policy.outputs.ra001a_durable_tracer_required != 'true' }}\n"
    )
    minio_cleanup_skip = (
        "      - name: Stop BVS MinIO S3 smoke container\n"
        "        if: ${{ always() && needs.ci-policy.outputs.ra001a_durable_tracer_required != 'true' }}\n"
    )
    for label, fragment in (
        ("dual managed-heavy request guard", tracer_dual_request_guard),
        ("AWS role preflight", tracer_role_preflight),
        ("AWS region preflight", tracer_region_preflight),
        ("MinIO setup skip", minio_skip),
        ("MinIO test skip", minio_test_skip),
        ("MinIO cleanup skip", minio_cleanup_skip),
    ):
        if fragment not in backtester_workflow:
            raise AssertionError(f"RA-001a tracer {label} is missing from the real workflow")

    for fragment, replacement, expected in (
        (
            tracer_dual_request_guard,
            "",
            "ci-policy job must include issue #789 and the RA-001a durable tracer are mutually exclusive managed-heavy requests",
        ),
        (
            tracer_role_preflight,
            "",
            'ci-policy job must include test -n "$RA001A_TRACER_ROLE_ARN"',
        ),
        (
            tracer_region_preflight,
            "",
            'ci-policy job must include test -n "$RA001A_TRACER_REGION"',
        ),
        (
            minio_skip,
            "      - name: Setup BVS MinIO S3 smoke\n",
            "RA-001a tracer-only dispatch must skip the unrelated MinIO smoke",
        ),
        (
            minio_test_skip,
            "      - name: test BVS MinIO S3 smoke\n",
            "RA-001a tracer-only dispatch must skip the unrelated MinIO smoke",
        ),
        (
            minio_cleanup_skip,
            "      - name: Stop BVS MinIO S3 smoke container\n        if: always()\n",
            "RA-001a tracer-only dispatch must skip unrelated MinIO cleanup",
        ),
        (
            archive_job_timeout,
            "    timeout-minutes: 120\n",
            "backtester test-archive timeout-minutes must use the trusted ci-policy output",
        ),
        (
            issue_789_job_timeout,
            "    timeout-minutes: 120\n",
            "backtester issue_789 timeout-minutes must use the trusted ci-policy output",
        ),
        (
            timeout_output_capture,
            "",
            "ci-policy job must capture trusted policy output before publication",
        ),
        (
            timeout_output_validation,
            '          cat "$policy_output" >> "$GITHUB_OUTPUT"\n',
            "ci-policy job must fail closed on missing or malformed timeout outputs",
        ),
        (
            tracer_checkout_policy_env,
            "",
            "RA-001a checkout cleanliness policy must come from trusted TOML outputs",
        ),
        (
            tracer_no_tests_fail,
            "            --run-ignored ignored-only \\\n",
            "RA-001a tracer must fail when its exact ignored test selection is empty",
        ),
    ):
        mutated = replace_once(backtester_workflow, fragment, replacement)
        mutation_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": mutated}
        )
        if not any(expected in error for error in mutation_errors):
            raise AssertionError(
                f"backtester governance mutation did not fail with {expected!r}: {mutation_errors}"
            )

    zero_accepting_timeout_validation = replace_once(
        backtester_workflow,
        "$2 ~ /^[1-9][0-9]*$/",
        "$2 ~ /^[0-9][0-9]*$/",
    )
    zero_accepting_timeout_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": zero_accepting_timeout_validation}
    )
    if not any(
        "ci-policy job must fail closed on missing or malformed timeout outputs" in error
        for error in zero_accepting_timeout_errors
    ):
        raise AssertionError(
            "backtester governance must reject timeout validation that accepts zero"
        )

    early_timeout_publication = replace_once(
        backtester_workflow,
        '          | tee "$policy_output"\n'
        "          for positive_policy_key in \\\n",
        '          | tee "$policy_output"\n'
        '          cat "$policy_output" >> "$GITHUB_OUTPUT"\n'
        "          for positive_policy_key in \\\n",
    )
    early_timeout_publication_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": early_timeout_publication}
    )
    if not any(
        "ci-policy job must validate timeout outputs before publishing them" in error
        for error in early_timeout_publication_errors
    ):
        raise AssertionError(
            "backtester governance must reject timeout publication before validation"
        )

    unbounded_wall_workflow = replace_once(
        backtester_workflow,
        tracer_timeout,
        "            just bte-test-archive-run \\\n",
    )
    unbounded_wall_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unbounded_wall_workflow}
    )
    if not any(
        "RA-001a tracer must enforce its TOML-owned aggregate wall-time envelope" in error
        for error in unbounded_wall_errors
    ):
        raise AssertionError("backtester governance must reject an unbounded aggregate tracer run")

    full_suite_tracer_workflow = replace_once(
        backtester_workflow,
        tracer_test_skip,
        "      - name: test\n",
    )
    full_suite_tracer_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": full_suite_tracer_workflow}
    )
    if not any(
        "RA-001a tracer-only dispatch must skip the unrelated partition suite" in error
        for error in full_suite_tracer_errors
    ):
        raise AssertionError("backtester governance must reject an unrelated full suite on tracer dispatch")

    unbounded_bytes_workflow = replace_once(
        backtester_workflow,
        tracer_byte_env,
        "",
    )
    unbounded_bytes_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unbounded_bytes_workflow}
    )
    if not any(
        "RA-001a tracer must receive its TOML-owned aggregate source-byte envelope" in error
        for error in unbounded_bytes_errors
    ):
        raise AssertionError("backtester governance must reject a tracer without an aggregate byte cap")

    tracer_external_root = (
        '            echo "RUST_VERIFICATION_ROOT_BASE=$RUNNER_TEMP/.rust-verification"\n'
    )
    tracer_external_archive = (
        '            echo "BVS_NEXTEST_ARCHIVE_PATH=$RUNNER_TEMP/bvs-nextest-archive/'
        'bvs-nextest-archive.tar.zst"\n'
    )
    tracer_external_sidecars = (
        '            echo "BVS_BIN_SIDECARS_PATH=$RUNNER_TEMP/bvs-nextest-archive/'
        'bvs-bin-sidecars.tar.gz"\n'
    )
    tracer_route_export = '          } >> "$GITHUB_ENV"\n'
    tracer_gate_intent = (
        '            tracer_verdict_args=(--ra001a-durable-tracer-required "'
        "${{ needs.ci-policy.outputs.ra001a_durable_tracer_required || 'false' }}"
        '")\n'
    )
    tracer_gate_probe = (
        '          if python3 "$verdict_script" check-backtester-gate --help | grep -q -- '
        '"--ra001a-durable-tracer-required"; then\n'
    )
    tracer_gate_expansion = '            "${tracer_verdict_args[@]}" \\\n'
    for label, fragment in (
        ("external Rust target root", tracer_external_root),
        ("external nextest archive", tracer_external_archive),
        ("external binary sidecars", tracer_external_sidecars),
        ("GITHUB_ENV output export", tracer_route_export),
        ("gate intent propagation", tracer_gate_intent),
        ("trusted-base gate capability probe", tracer_gate_probe),
        ("gate argument expansion", tracer_gate_expansion),
    ):
        if fragment not in backtester_workflow:
            raise AssertionError(f"RA-001a tracer {label} is missing from the real workflow")

    checkout_target_workflow = replace_once(
        backtester_workflow,
        tracer_external_root,
        '            echo "RUST_VERIFICATION_ROOT_BASE=$GITHUB_WORKSPACE/.rust-verification"\n',
    )
    checkout_target_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": checkout_target_workflow}
    )
    if not any(
        "RA-001a tracer test-archive Rust target must remain outside the checkout" in error
        for error in checkout_target_errors
    ):
        raise AssertionError("backtester governance must reject a checkout-local tracer Rust target")

    checkout_archive_workflow = replace_once(
        backtester_workflow,
        tracer_external_archive,
        '            echo "BVS_NEXTEST_ARCHIVE_PATH=$GITHUB_WORKSPACE/.nextest-archive/'
        'bvs-nextest-archive.tar.zst"\n',
    )
    checkout_archive_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": checkout_archive_workflow}
    )
    if not any(
        "RA-001a tracer archive payloads must remain outside the checkout" in error
        for error in checkout_archive_errors
    ):
        raise AssertionError("backtester governance must reject a checkout-local tracer archive")

    checkout_sidecars_workflow = replace_once(
        backtester_workflow,
        tracer_external_sidecars,
        '            echo "BVS_BIN_SIDECARS_PATH=$GITHUB_WORKSPACE/.nextest-archive/'
        'bvs-bin-sidecars.tar.gz"\n',
    )
    checkout_sidecars_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": checkout_sidecars_workflow}
    )
    if not any(
        "RA-001a tracer archive payloads must remain outside the checkout" in error
        for error in checkout_sidecars_errors
    ):
        raise AssertionError("backtester governance must reject checkout-local tracer sidecars")

    unexported_paths_workflow = replace_once(
        backtester_workflow,
        tracer_route_export,
        "          }\n",
    )
    unexported_paths_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unexported_paths_workflow}
    )
    if not any(
        "RA-001a tracer output routing must export through GITHUB_ENV" in error
        for error in unexported_paths_errors
    ):
        raise AssertionError("backtester governance must reject unexported external paths")

    unrouted_gate_workflow = replace_once(backtester_workflow, tracer_gate_intent, "")
    unrouted_gate_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unrouted_gate_workflow}
    )
    if not any(
        "RA-001a tracer intent must reach the backtester gate verdict" in error
        for error in unrouted_gate_errors
    ):
        raise AssertionError("backtester governance must reject unconsumed tracer gate intent")

    unprobed_gate_workflow = replace_once(
        backtester_workflow,
        tracer_gate_probe,
        "          if true; then\n",
    )
    unprobed_gate_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": unprobed_gate_workflow}
    )
    if not any(
        "RA-001a tracer intent must reach the backtester gate verdict" in error
        for error in unprobed_gate_errors
    ):
        raise AssertionError("backtester governance must reject an unprobed trusted-base gate")

    # (i) merge_group policy value flipped away from required proof → config contract error.
    flipped_config = ci_provenance_config_fixture().replace(
        'merge_group = "full"', 'merge_group = "iteration"'
    )
    if flipped_config == ci_provenance_config_fixture():
        raise AssertionError("merge_group policy fixture fragment not found")
    error = runner_config_load_error(flipped_config)
    if "ci_provenance.policy.merge_group is proof-affecting" not in error:
        raise AssertionError(f"expected merge_group policy contract error, got: {error!r}")

    # (ii-a) merge_group trigger removed from ci.yml → CI workflow error.
    ci_without_merge_group = replace_once(
        ci_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    ci_errors = verifier.verify_workflow(ci_without_merge_group)
    if not any("on must define merge_group for merge queue full CI" in error for error in ci_errors):
        raise AssertionError(f"expected ci.yml merge_group trigger error, got: {ci_errors}")

    # (ii-b) merge_group trigger removed from actionlint.yml → actionlint error.
    actionlint_without_merge_group = replace_once(
        actionlint_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    actionlint_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_without_merge_group}
    )
    if not any(
        "on must define merge_group for merge queue" in error for error in actionlint_errors
    ):
        raise AssertionError(
            f"expected actionlint.yml merge_group trigger error, got: {actionlint_errors}"
        )

    # (ii-c) merge_group trigger removed from backtester-ci.yml → Backtester CI error.
    backtester_without_merge_group = replace_once(
        backtester_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    backtester_trigger_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_merge_group}
    )
    if not any(
        "on must define merge_group for merge queue" in error for error in backtester_trigger_errors
    ):
        raise AssertionError(
            f"expected backtester-ci.yml merge_group trigger error, got: {backtester_trigger_errors}"
        )

    # Backtester detect must force proof lanes on merge_group. A no-op required
    # gate counts as passing and would poison the live queue evidence.
    backtester_without_detector_arm = replace_once(
        backtester_workflow,
        '          elif [[ "${{ github.event_name }}" == "merge_group" ]]; then\n'
        "            # A skipped required gate counts as passing, so queue validation must run proof lanes.\n"
        "            # The merge queue is proof-bearing, so avoid opaque archive reuse when this path\n"
        "            # cannot use the pull_request bootstrap diff.\n"
        '            echo "merge_group event; treating crate as changed"\n'
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        '            exit 0\n',
        "",
    )
    backtester_detector_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_detector_arm}
    )
    if not any(
        "backtester detect must force bvs_changed=true for merge_group" in error
        for error in backtester_detector_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group detector error, got: {backtester_detector_errors}"
        )

    backtester_detector_without_exit = replace_once(
        backtester_workflow,
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        "            exit 0\n"
        "          fi\n",
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        "          fi\n",
    )
    backtester_detector_exit_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_detector_without_exit}
    )
    if not any(
        "backtester detect must force bvs_changed=true for merge_group" in error
        for error in backtester_detector_exit_errors
    ):
        raise AssertionError(
            "expected backtester merge_group detector short-circuit error, "
            f"got: {backtester_detector_exit_errors}"
        )

    # Detector must force build on merge_group (a skipped required build is a hole).
    ci_without_detector_arm = replace_once(
        ci_workflow,
        '          elif [[ "${{ github.event_name }}" == "merge_group" ]]; then\n',
        "",
    )
    detector_errors = verifier.verify_workflow(ci_without_detector_arm)
    if not any(
        "detector must force build_required=true for merge_group full CI" in error
        for error in detector_errors
    ):
        raise AssertionError(f"expected merge_group detector guard error, got: {detector_errors}")

    # Concurrency group must match an approved merge_group-safe form and must
    # not cancel merge_group runs.
    ci_without_concurrency_arm = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n        && format('mq-{0}', github.ref)\n",
        "",
    )
    concurrency_errors = verifier.verify_workflow(ci_without_concurrency_arm)
    if not any(
        "approved merge_group-safe form" in error
        for error in concurrency_errors
    ):
        raise AssertionError(f"expected merge_group concurrency error, got: {concurrency_errors}")

    ci_cancelling_merge_group = replace_once(
        ci_workflow,
        "        || github.event_name == 'workflow_dispatch' }}",
        "        || github.event_name == 'workflow_dispatch'\n        || github.event_name == 'merge_group' }}",
    )
    cancel_errors = verifier.verify_workflow(ci_cancelling_merge_group)
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_errors
    ):
        raise AssertionError(f"expected merge_group cancel-scope error, got: {cancel_errors}")

    backtester_without_concurrency_arm = replace_once(
        backtester_workflow,
        "        || github.event_name == 'merge_group'\n        && format('bvs-mq-{0}', github.ref)\n",
        "",
    )
    backtester_concurrency_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_concurrency_arm}
    )
    if not any(
        "approved merge_group-safe form" in error
        for error in backtester_concurrency_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group concurrency error, got: {backtester_concurrency_errors}"
        )

    backtester_cancelling_merge_group = replace_once(
        backtester_workflow,
        "        || (github.event_name == 'workflow_dispatch'\n"
        "            && inputs.ra001a_durable_tracer != true) }}",
        "        || (github.event_name == 'workflow_dispatch'\n"
        "            && inputs.ra001a_durable_tracer != true)\n"
        "        || github.event_name == 'merge_group' }}",
    )
    backtester_cancel_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_cancelling_merge_group}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in backtester_cancel_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group cancel-scope error, got: {backtester_cancel_errors}"
        )

    # Decoupled merge_group arm (ci.yml): a merge_group arm must be caught even
    # when 'mq-{0}'/'github.ref' still appear elsewhere. Swap the merge_group and
    # workflow_dispatch format strings so both substrings remain present but the
    # merge_group arm no longer keys on format('mq-{0}', github.ref). The allowlist
    # rejects it because the resulting group is not an approved form. (Regression
    # coverage: the prior expression-analysis verifier rejected this too — NOT a
    # gap the allowlist uniquely closes.)
    ci_fail_open = replace_once(
        ci_workflow,
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && format('{0}-dispatch-iteration', github.ref_name)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n",
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('{0}-dispatch-iteration', github.ref_name)\n",
    )
    if ci_fail_open == ci_workflow:
        raise AssertionError("merge_group fail-open fixture fragment not found in ci.yml")
    fail_open_errors = verifier.verify_workflow(ci_fail_open)
    if not any(
        "approved merge_group-safe form" in error
        for error in fail_open_errors
    ):
        raise AssertionError(
            f"merge_group concurrency allowlist must reject a decoupled arm, got: {fail_open_errors}"
        )

    # actionlint concurrency must also isolate merge_group (the reviewer-flagged
    # class gap: only ci.yml's concurrency was contract-checked). Removing
    # actionlint's merge_group concurrency arm must be reported.
    actionlint_no_concurrency_arm = replace_once(
        actionlint_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n",
        "",
    )
    if actionlint_no_concurrency_arm == actionlint_workflow:
        raise AssertionError("actionlint merge_group concurrency fixture fragment not found")
    actionlint_concurrency_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_no_concurrency_arm}
    )
    if not any(
        "approved merge_group-safe form" in error
        for error in actionlint_concurrency_errors
    ):
        raise AssertionError(
            f"expected actionlint merge_group concurrency error, got: {actionlint_concurrency_errors}"
        )

    # actionlint cancel-in-progress must never cancel merge_group queue runs.
    actionlint_cancel_merge_group = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "      || github.event_name == 'merge_group' }}",
    )
    if actionlint_cancel_merge_group == actionlint_workflow:
        raise AssertionError("actionlint cancel-in-progress fixture fragment not found")
    actionlint_cancel_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_merge_group}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in actionlint_cancel_errors
    ):
        raise AssertionError(
            f"expected actionlint merge_group cancel-scope error, got: {actionlint_cancel_errors}"
        )

    # cancel-in-progress: true cancels merge_group queue runs while naming no
    # event literally — the old bare-substring check missed it. (Reviewer-flagged
    # fail-open class: GPT/GLM.) The positive allowlist must reject it.
    actionlint_cancel_true = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: true",
    )
    if actionlint_cancel_true == actionlint_workflow:
        raise AssertionError("actionlint cancel-in-progress: true fixture fragment not found")
    cancel_true_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_true}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_true_errors
    ):
        raise AssertionError(
            f"cancel-in-progress: true must be rejected for merge_group, got: {cancel_true_errors}"
        )

    # A negation true for the queue ref (!= 'push') cancels the run while naming
    # no event literally — also fail-open under a substring deny-list.
    actionlint_cancel_negation = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: ${{ github.event_name != 'push' }}",
    )
    if actionlint_cancel_negation == actionlint_workflow:
        raise AssertionError("actionlint cancel negation fixture fragment not found")
    cancel_negation_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_negation}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_negation_errors
    ):
        raise AssertionError(
            f"cancel negation true for merge_group must be rejected, got: {cancel_negation_errors}"
        )

    # Decoy-after-fallback (ci.yml): the real merge_group arm is decoupled to a
    # shared key, but a dead keyed arm sits after the always-true fallback (which
    # GitHub's `||` never reaches). The allowlist rejects it because the decoupled
    # group expression is not an approved form. (Regression coverage: the prior
    # expression-analysis verifier rejected this too — a single .search() for the
    # keyed arm would have passed, but the count-based check did not — NOT a gap
    # the allowlist uniquely closes.)
    ci_decoy_after_fallback = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('shared-key')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_decoy_after_fallback == ci_workflow:
        raise AssertionError("ci.yml decoy-after-fallback fixture fragment not found")
    decoy_errors = verifier.verify_workflow(ci_decoy_after_fallback)
    if not any(
        "approved merge_group-safe form" in error
        for error in decoy_errors
    ):
        raise AssertionError(
            f"a decoupled merge_group arm hidden behind a keyed decoy must be rejected, got: {decoy_errors}"
        )

    # Index-syntax escape (ci.yml): the real merge_group arm selects the event
    # via github['event_name'] and uses a shared key, with a canonical keyed
    # decoy after the fallback. A counter keyed on the literal `github.event_name
    # == 'merge_group'` token never counts the index arm, so the count stays
    # balanced and it slips through — the allowlist rejects it. (Differential: the
    # prior expression-analysis verifier leaked this; the allowlist uniquely
    # closes it.)
    ci_index_syntax_escape = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github['event_name'] == 'merge_group'\n"
        "        && format('shared-key')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_index_syntax_escape == ci_workflow:
        raise AssertionError("ci.yml index-syntax escape fixture fragment not found")
    index_errors = verifier.verify_workflow(ci_index_syntax_escape)
    if not any(
        "approved merge_group-safe form" in error
        for error in index_errors
    ):
        raise AssertionError(
            f"an unkeyed merge_group arm using github['event_name'] must be rejected, got: {index_errors}"
        )

    # Ref-shape escape (ci.yml): an arm true for the queue ref
    # (startsWith(github.ref, 'refs/heads/gh-readonly-queue')) with a shared key
    # is placed before the canonical arm, so it wins under merge_group. It names
    # no event literally, so a token counter never sees it; the allowlist rejects
    # it. (Differential: the prior expression-analysis verifier leaked this; the
    # allowlist uniquely closes it.)
    ci_ref_shape_escape = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || startsWith(github.ref, 'refs/heads/gh-readonly-queue')\n"
        "        && format('shared-key')\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_ref_shape_escape == ci_workflow:
        raise AssertionError("ci.yml ref-shape escape fixture fragment not found")
    ref_shape_errors = verifier.verify_workflow(ci_ref_shape_escape)
    if not any(
        "approved merge_group-safe form" in error
        for error in ref_shape_errors
    ):
        raise AssertionError(
            f"an unkeyed arm true for the queue ref must be rejected, got: {ref_shape_errors}"
        )

    # Literal-string spoof (ci.yml): the merge_group arm's value is a constant
    # key that merely contains the text 'github.ref', so every queue entry gets
    # the same group. A naive ref-isolation check matching the bare token would be
    # fooled; the allowlist rejects it because the constant group is not an
    # approved form. (Regression coverage: the prior expression-analysis verifier
    # also rejected this form — it required github.ref as a format() placeholder
    # arg — so this is NOT a gap the allowlist uniquely closes; see the
    # load-bearing allowlist guard below for what is proven.)
    ci_literal_spoof = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-github.ref-static')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_literal_spoof == ci_workflow:
        raise AssertionError("ci.yml literal-spoof fixture fragment not found")
    literal_errors = verifier.verify_workflow(ci_literal_spoof)
    if not any(
        "approved merge_group-safe form" in error
        for error in literal_errors
    ):
        raise AssertionError(
            f"a merge_group arm keyed on a constant string containing 'github.ref' must be "
            f"rejected, got: {literal_errors}"
        )

    # github.ref wrapped in a constant-collapsing function
    # (startsWith/endsWith/contains) yields the same key for every queue ref. The
    # allowlist rejects it because the normalized group is not an approved form.
    # (Regression coverage: the prior expression-analysis verifier also rejected
    # this — the merge_group arm's format() arg was startsWith(...), not the bare
    # github.ref it required — so, like literal_spoof, it is NOT a gap the
    # allowlist uniquely closes. The forms the allowlist DOES uniquely close
    # against expression analysis are index_syntax/ref_shape/amp_literal/
    # gate_literal; the guard below proves the allowlist is the sole gate for all
    # of them without depending on which historical check caught which.)
    ci_collapse = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', startsWith(github.ref, 'refs/heads/gh-readonly-queue'))\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_collapse == ci_workflow:
        raise AssertionError("ci.yml collapse fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_collapse)
    ):
        raise AssertionError("a github.ref wrapped in startsWith() must be rejected")

    # `&&` inside a string literal mis-splits a naive value/condition parse; the
    # whole literal is one constant key to GitHub. (Differential: the prior
    # expression-analysis verifier leaked this; the allowlist uniquely closes it.)
    ci_amp_literal = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || format('mq-static && github.ref ', 'x')\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_amp_literal == ci_workflow:
        raise AssertionError("ci.yml amp-in-literal fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_amp_literal)
    ):
        raise AssertionError("an && hidden inside a string literal must be rejected")

    # Event-gate text inside a string literal is not a real conjunct; the arm
    # still wins under merge_group with a shared static key. (Differential: the
    # prior expression-analysis verifier leaked this; the allowlist uniquely
    # closes it.)
    ci_gate_literal = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || format(\"skip github.event_name == 'pull_request'\", github.ref) && 'mq-shared-static-group'\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_gate_literal == ci_workflow:
        raise AssertionError("ci.yml gate-in-literal fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_gate_literal)
    ):
        raise AssertionError("a gate hidden inside a string literal must be rejected")

    # --- Load-bearing proof for the group allowlist (differential) ---
    # Every merge_group group-expression mutation above is a NON-APPROVED group
    # form, and the positive allowlist rejects each one. Most resolve to a shared
    # or constant group that is genuinely unsafe under merge_group; fail_open_swap
    # is the exception — its merge_group arm keys on github.ref_name, which is
    # unique per queue entry (gh-readonly-queue/<base>/pr-N-<sha>), so it would in
    # fact isolate, yet it is still rejected because it is not the exact approved
    # form. That is the allowlist's whole point: it is fail-closed on any
    # non-approved form and never tries to decide whether a novel form happens to
    # be safe. Stub the allowlist branch back out (pre-rework behavior: cancel
    # check only) and every one must stop being rejected — proving the allowlist
    # is the sole load-bearing gate, not a vacuous assertion. (Some of these forms
    # were ALSO caught by the prior expression-analysis verifier and are kept as
    # regression coverage; the allowlist's value is that it rejects all of them
    # without depending on which historical check caught which.) load_verifier()
    # returns a fresh module, but restore anyway so the patch cannot leak.
    allowlist_gated_group_mutations = [
        ("fail_open_swap", ci_fail_open),
        ("decoy_after_fallback", ci_decoy_after_fallback),
        ("index_syntax_escape", ci_index_syntax_escape),
        ("ref_shape_escape", ci_ref_shape_escape),
        ("literal_spoof", ci_literal_spoof),
        ("collapse", ci_collapse),
        ("amp_literal", ci_amp_literal),
        ("gate_literal", ci_gate_literal),
    ]
    original_group_check = verifier.merge_group_concurrency_errors
    try:
        verifier.merge_group_concurrency_errors = (
            lambda group_text, cancel_text: (
                []
                if verifier.cancel_in_progress_is_merge_group_safe(cancel_text)
                else ["cancel-in-progress must not cancel merge_group queue validations"]
            )
        )
        for label, mutated in allowlist_gated_group_mutations:
            if any(
                "approved merge_group-safe form" in error
                for error in verifier.verify_workflow(mutated)
            ):
                raise AssertionError(
                    f"differential: {label} must no longer draw the allowlist error "
                    "once the group allowlist is stubbed out (else the allowlist "
                    "guard proves nothing)"
                )
    finally:
        verifier.merge_group_concurrency_errors = original_group_check

    # Duplicate top-level group: key — GitHub takes the last (a constant). The
    # extractor joins both group: lines, so the normalized text is not approved.
    dup_block = (
        "concurrency:\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request' && format('pr-{0}', github.event.number) "
        "|| github.event_name == 'merge_group' && format('mq-{0}', github.ref) "
        "|| format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  group: ci-shared-merge-queue\n"
        "  cancel-in-progress: false\n"
    )
    dup_split = verifier.concurrency_group_and_cancel(dup_block)
    if dup_split is None:
        raise AssertionError("duplicate group: block did not parse")
    dup_errors = verifier.merge_group_concurrency_errors(*dup_split)
    if not any("approved merge_group-safe form" in error for error in dup_errors):
        raise AssertionError(f"a duplicate group: key must be rejected, got: {dup_errors}")

    # Reversed key order (actionlint.yml): cancel-in-progress written before
    # group. The split must bucket by key, not by first cancel occurrence;
    # otherwise the whole group expression is misread as cancel text and a valid
    # block draws a spurious "must key merge_group" error.
    actionlint_reversed = replace_once(
        actionlint_workflow,
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request'\n"
        "        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)\n"
        "        || github.event_name == 'pull_request'\n"
        "        && format('pr-{0}', github.event.number)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  # cancel-in-progress is true only for ordinary PR runs; merge_group and Mergify\n"
        "  # proof PR validations must never be cancelled.\n"
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request'\n"
        "        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)\n"
        "        || github.event_name == 'pull_request'\n"
        "        && format('pr-{0}', github.event.number)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if actionlint_reversed == actionlint_workflow:
        raise AssertionError("actionlint reversed key-order fixture fragment not found")
    reversed_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_reversed}
    )
    if any(
        "merge_group" in error and "actionlint.yml" in error
        for error in reversed_errors
    ):
        raise AssertionError(
            f"a valid block with cancel-in-progress before group must not draw a spurious "
            f"merge_group concurrency error, got: {reversed_errors}"
        )

    # --- Job-level concurrency fail-open (round-3 adversarial pass) ---
    # GitHub evaluates job-level `concurrency:` in addition to the workflow-level
    # block, so a shared/cancelling job-level group on a required merge_group job
    # collapses queue entries even when the workflow-level group is allowlist-safe.
    # actionlint does NOT catch this (verified: exit 0), so the verifier must own
    # it. (Duplicate top-level `concurrency:` keys are deliberately NOT re-detected
    # here: actionlint — a required merge_group check this verifier already
    # enforces — rejects them in every form, block/flow/quoted, verified exit 1;
    # see merge_group_concurrency_workflow_errors for the single-source rationale
    # and the liveness-only residual.)

    # (a) Job-level concurrency on real actionlint.yml — a shared/cancelling
    #     job-level group collapses queue entries even with a safe workflow block.
    #     Exercises the verify_merge_group_concurrency entry point.
    actionlint_job_level = replace_once(
        actionlint_workflow,
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    steps:",
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n"
        "    concurrency:\n      group: actionlint-shared\n      cancel-in-progress: true\n"
        "    steps:",
    )
    if actionlint_job_level == actionlint_workflow:
        raise AssertionError("actionlint job-level concurrency fixture fragment not found")
    job_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_job_level}
    )
    if not any("must not define job-level concurrency" in error for error in job_errors):
        raise AssertionError(
            f"job-level concurrency in a merge_group workflow must be rejected, got: {job_errors}"
        )

    # (b) Job-level concurrency on real ci.yml — exercises the verify_pr_concurrency
    #     entry point, confirming both merge_group concurrency entry points are
    #     wired to the job-level check.
    ci_job_level = replace_once(
        ci_workflow,
        "  build:\n    name: build\n    needs: [ci-policy, detector]\n    if:",
        "  build:\n    name: build\n    needs: [ci-policy, detector]\n"
        "    concurrency:\n      group: ci-build-shared\n      cancel-in-progress: true\n"
        "    if:",
    )
    if ci_job_level == ci_workflow:
        raise AssertionError("ci.yml job-level concurrency fixture fragment not found")
    if not any(
        "must not define job-level concurrency" in error
        for error in verifier.verify_workflow(ci_job_level)
    ):
        raise AssertionError("job-level concurrency in ci.yml must be rejected")

    # (c) False-positive guard: `concurrency:` appearing as run-block text (deeper
    #     than the job-key indentation) must NOT be flagged — only a real
    #     job-level key counts. Proves the indentation discrimination is
    #     load-bearing (a naive substring scan would wrongly reject this).
    job_run_block_text = (
        "name: actionlint\non:\n  merge_group:\n  pull_request:\n"
        "concurrency:\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request' && format('pr-{0}', github.event.number)\n"
        "    || github.event_name == 'merge_group' && format('mq-{0}', github.ref)\n"
        "    || format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  cancel-in-progress: false\n"
        "jobs:\n  lint:\n    runs-on: ubuntu-latest\n"
        "    steps:\n      - run: |\n          echo 'concurrency: not-a-real-key'\n"
    )
    if verifier.jobs_with_job_level_concurrency(job_run_block_text):
        raise AssertionError(
            "run-block text 'concurrency:' must not be misread as a job-level concurrency key"
        )

    # (d) Differential proof: stub the whole-workflow check back out (the pre-fix
    #     behavior) and the bypass passes — proving the job-level check is
    #     load-bearing, not vacuous. load_verifier() returns a fresh module, but
    #     restore anyway so the patch cannot leak.
    original_whole_workflow = verifier.merge_group_concurrency_workflow_errors
    try:
        verifier.merge_group_concurrency_workflow_errors = lambda _text: []
        job_stubbed = verifier.verify_repo_automation_texts(
            {".github/workflows/actionlint.yml": actionlint_job_level}
        )
        if any("must not define job-level concurrency" in error for error in job_stubbed):
            raise AssertionError(
                "differential sanity: the job-level error must vanish once the "
                "job-level check is stubbed out (else the test proves nothing)"
            )
    finally:
        verifier.merge_group_concurrency_workflow_errors = original_whole_workflow

def assert_gate_policy_truth_table_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cases = [
        (
            "gate needs ci-policy",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "ci-policy")),
        ),
        (
            "gate needs nextest-fingerprint",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "nextest-fingerprint")),
        ),
        (
            "gate needs test-archive",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "test-archive")),
        ),
        (
            "gate name must come from ci-policy gate_name output",
            replace_once(workflow, GATE_NAME, "name: gate"),
        ),
        (
            "gate shared verdict call must include --job ci-policy=${{ needs.ci-policy.result }}",
            replace_once(
                workflow,
                "--job ci-policy=${{ needs.ci-policy.result }}",
                "--job ci-policy=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --policy-path",
            replace_once(workflow, '--policy-path "${{ needs.ci-policy.outputs.ci_policy_path }}"', '--policy-path "full"'),
        ),
        (
            "gate shared verdict call must include --expected-event-class",
            replace_once(
                workflow,
                '--expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}"',
                '--expected-event-class "iteration"',
            ),
        ),
        (
            "gate shared verdict call must include --job nextest-fingerprint=${{ needs.nextest-fingerprint.result }}",
            replace_once(
                workflow,
                "--job nextest-fingerprint=${{ needs.nextest-fingerprint.result }}",
                "--job nextest-fingerprint=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --job test-archive=${{ needs.test-archive.result }}",
            replace_once(
                workflow,
                "--job test-archive=${{ needs.test-archive.result }}",
                "--job test-archive=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
            replace_once(
                workflow,
                "--job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
                "--job same-sha-main-evidence=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --ignore-emit-failure",
            replace_once(
                workflow,
                '--ignore-emit-failure "${{ needs.ci-policy.outputs.ignore_emit_failure }}"',
                '--ignore-emit-failure "false"',
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")

def assert_flaky_detection_workflows_are_split_without_mode_gates() -> None:
    verifier = load_verifier()
    full_workflow = repo_workflow_text(".github/workflows/flaky-test-detection.yml")
    smoke_workflow = repo_workflow_text(".github/workflows/flaky-test-smoke.yml")
    if "schedule:" in full_workflow:
        raise AssertionError("flaky-test-detection.yml must remain manual-only")
    if full_workflow.count('exit "$rc"') != 3:
        raise AssertionError("flaky-test-detection.yml run steps must exit with the captured rc")
    if "workflow_dispatch:" not in smoke_workflow:
        raise AssertionError("flaky-test-smoke.yml must remain schedule-driven and manually dispatchable")
    for workflow_path, workflow in (
        (".github/workflows/flaky-test-detection.yml", full_workflow),
        (".github/workflows/flaky-test-smoke.yml", smoke_workflow),
    ):
        forbidden_fragments = (
            "mode:",
            "inputs.mode",
            "github.event_name",
            "fromJSON(",
            "if: ${{ github.event_name",
            "${{ steps.setup.outputs.managed_target_dir }}/nextest/default/junit-unit-",
            "${{ steps.crate_target.outputs.dir }}/nextest/default/junit-unit-",
        )
        for fragment in forbidden_fragments:
            if fragment in workflow:
                raise AssertionError(f"{workflow_path} must not contain {fragment!r}")
    full_fragments = (
        "workflow_dispatch:",
        "flaky-detection-rust-root:",
        "flaky-detection-rust-backtester:",
        "flaky-detection-rust-backtester-issue-789:",
    )
    for fragment in full_fragments:
        if fragment not in full_workflow:
            raise AssertionError(f"flaky-test-detection.yml missing {fragment!r}")
    full_jobs = verifier.parse_jobs(full_workflow)
    full_root_runs = inline_matrix_values(full_jobs["flaky-detection-rust-root"], "run_number")
    full_backtester_runs = inline_matrix_values(full_jobs["flaky-detection-rust-backtester"], "run_number")
    full_backtester_shards = inline_matrix_values(full_jobs["flaky-detection-rust-backtester"], "shard")
    full_issue_runs = inline_matrix_values(full_jobs["flaky-detection-rust-backtester-issue-789"], "run_number")
    assert_no_inline_matrix_key(full_jobs["flaky-detection-rust-root"], "shard")
    assert_no_inline_matrix_key(full_jobs["flaky-detection-rust-backtester-issue-789"], "shard")
    if full_root_runs != (1, 2, 3, 4, 5):
        raise AssertionError("flaky-test-detection.yml root job must keep five repeat runs")
    if full_backtester_runs != (1, 2, 3, 4, 5):
        raise AssertionError("flaky-test-detection.yml backtester job must keep five repeat runs")
    if not one_indexed_sequence(full_backtester_shards):
        raise AssertionError("flaky-test-detection.yml backtester shard matrix must stay one-indexed and contiguous")
    if shard_partition_argument_denominators(full_jobs["flaky-detection-rust-backtester"]) != (len(full_backtester_shards),):
        raise AssertionError("flaky-test-detection.yml backtester shard partition denominator must match shard matrix length")
    if full_issue_runs != (1, 2, 3, 4, 5):
        raise AssertionError("flaky-test-detection.yml issue-789 job must keep five repeat runs")
    smoke_fragments = (
        "workflow_dispatch:",
        "schedule:",
        "cron: '0 */12 * * 1-5'",
        "flaky-smoke-rust-root:",
        "flaky-smoke-rust-backtester:",
        "flaky-smoke-rust-backtester-issue-789:",
    )
    for fragment in smoke_fragments:
        if fragment not in smoke_workflow:
            raise AssertionError(f"flaky-test-smoke.yml missing {fragment!r}")
    smoke_jobs = verifier.parse_jobs(smoke_workflow)
    smoke_root_runs = inline_matrix_values(smoke_jobs["flaky-smoke-rust-root"], "run_number")
    smoke_backtester_runs = inline_matrix_values(smoke_jobs["flaky-smoke-rust-backtester"], "run_number")
    smoke_backtester_shards = inline_matrix_values(smoke_jobs["flaky-smoke-rust-backtester"], "shard")
    smoke_issue_runs = inline_matrix_values(smoke_jobs["flaky-smoke-rust-backtester-issue-789"], "run_number")
    assert_no_inline_matrix_key(smoke_jobs["flaky-smoke-rust-root"], "shard")
    assert_no_inline_matrix_key(smoke_jobs["flaky-smoke-rust-backtester-issue-789"], "shard")
    if smoke_root_runs != (1,) or smoke_backtester_runs != (1,) or smoke_issue_runs != (1,):
        raise AssertionError("flaky-test-smoke.yml must keep one run per smoke job")
    if smoke_backtester_shards != full_backtester_shards:
        raise AssertionError(
            "flaky-test-smoke.yml backtester shard matrix must cover every flaky-test-detection.yml shard"
        )
    smoke_partition_denominators = shard_partition_argument_denominators(smoke_jobs["flaky-smoke-rust-backtester"])
    if smoke_partition_denominators != (len(smoke_backtester_shards),):
        raise AssertionError("flaky-test-smoke.yml backtester shard partition denominator must match shard matrix length")
    smoke_execution_count = (
        len(smoke_root_runs)
        + len(smoke_backtester_runs) * len(smoke_backtester_shards)
        + len(smoke_issue_runs)
    )
    expected_smoke_execution_count = 2 + len(full_backtester_shards)
    if smoke_execution_count != expected_smoke_execution_count:
        raise AssertionError(
            f"flaky-test-smoke.yml must keep {expected_smoke_execution_count} matrix executions, "
            f"got {smoke_execution_count}"
        )

def assert_flaky_detection_workflow_split_gaps_are_reported() -> None:
    verifier = load_verifier()
    full_workflow_name = ".github/workflows/flaky-test-detection.yml"
    smoke_workflow_name = ".github/workflows/flaky-test-smoke.yml"
    good_full_workflow = """name: Flaky Test Detection

on:
  workflow_dispatch:

jobs:
  flaky-detection-rust-root:
    strategy:
      matrix:
        run_number: [1, 2, 3, 4, 5]
    steps:
      - name: Run tests
        run: |
          rc=0
          set +e
          just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast
          rc=$?
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: nextest archive
        with:
          job_name: nextest archive
          report_path: "junit-*.xml"

  flaky-detection-rust-backtester:
    strategy:
      matrix:
        run_number: [1, 2, 3, 4, 5]
        shard: [1, 2, 3, 4]
    steps:
      - name: Setup BVS MinIO S3 smoke
        uses: ./.github/actions/setup-bvs-minio-s3-smoke

      - name: Run tests
        run: |
          rc=0
          set +e
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          rc=$?
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: bvs-test archive
        with:
          job_name: bvs-test archive
          report_path: "junit-*.xml"

      - name: Stop BVS MinIO S3 smoke container
        if: always()
        run: docker rm -f "$BVS_MINIO_CONTAINER_NAME" >/dev/null 2>&1 || true

  flaky-detection-rust-backtester-issue-789:
    strategy:
      matrix:
        run_number: [1, 2, 3, 4, 5]
    steps:
      - name: Run tests
        run: |
          rc=0
          set +e
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" issue_789_first_real_free_data_taker_pl
          rc=$?
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: bvs-test issue-789
        with:
          job_name: bvs-test issue-789
          report_path: "junit-*.xml"
"""
    good_smoke_workflow = """name: Flaky Test Smoke

on:
  workflow_dispatch:
  schedule:
    - cron: '0 */12 * * 1-5'

jobs:
  flaky-smoke-rust-root:
    strategy:
      matrix:
        run_number: [1]
    steps:
      - name: Run tests
        run: |
          rc=0
          set +e
          just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast 2>&1 | tee -a "$log"
          rc="${PIPESTATUS[0]}"
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: nextest archive
        with:
          job_name: nextest archive
          report_path: "junit-*.xml"

  flaky-smoke-rust-backtester:
    strategy:
      matrix:
        run_number: [1]
        shard: [1, 2, 3, 4]
    steps:
      - name: Setup BVS MinIO S3 smoke
        uses: ./.github/actions/setup-bvs-minio-s3-smoke

      - name: Run tests
        run: |
          rc=0
          set +e
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl 2>&1 | tee -a "$log"
          rc="${PIPESTATUS[0]}"
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: bvs-test archive
        with:
          job_name: bvs-test archive
          report_path: "junit-*.xml"

      - name: Stop BVS MinIO S3 smoke container
        if: always()
        run: docker rm -f "$BVS_MINIO_CONTAINER_NAME" >/dev/null 2>&1 || true

  flaky-smoke-rust-backtester-issue-789:
    strategy:
      matrix:
        run_number: [1]
    steps:
      - name: Run tests
        run: |
          rc=0
          set +e
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" issue_789_first_real_free_data_taker_pl 2>&1 | tee -a "$log"
          rc="${PIPESTATUS[0]}"
          set -e
          printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' "$rc" >> "$GITHUB_ENV"
          exit "$rc"

      - name: Stage JUnit report
        if: success() || failure()
        run: |
          if [[ -f "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi

      - name: Upload test results to Mergify
        if: success() || failure()
        uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14
        env:
          MERGIFY_TEST_JOB_NAME: bvs-test issue-789
        with:
          job_name: bvs-test issue-789
          report_path: "junit-*.xml"
"""
    root_stage_step = """        if: success() || failure()
        run: |
          if [[ -f "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi"""
    root_stage_step_with_fallback = """        if: success() || failure()
        run: |
          report="target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
          staged="junit-unit-${{ matrix.run_number }}.xml"
          if [[ -f "$report" ]]; then
            cp "$report" "$staged"
          else
            python3 - > "$staged" <<'PY'
          import os
          import xml.sax.saxutils as sax
          rc = sax.escape(os.environ.get("MERGIFY_TEST_EXIT_CODE", "unknown"))
          print('<?xml version="1.0" encoding="UTF-8"?>')
          print('<testsuite name="nextest-preflight" tests="1" failures="1">')
          print('<testcase classname="ci" name="missing-nextest-junit">')
          print(f'<failure message="nextest JUnit report was not produced">MERGIFY_TEST_EXIT_CODE={rc}; see the Run tests log.</failure>')
          print('</testcase></testsuite>')
          PY
          fi"""
    bvs_stage_step = """        if: success() || failure()
        run: |
          if [[ -f "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" ]]; then
            cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml"
          fi"""
    bvs_stage_step_with_fallback = """        if: success() || failure()
        run: |
          report="crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
          staged="junit-unit-${{ matrix.run_number }}.xml"
          if [[ -f "$report" ]]; then
            cp "$report" "$staged"
          else
            python3 - > "$staged" <<'PY'
          import os
          import xml.sax.saxutils as sax
          rc = sax.escape(os.environ.get("MERGIFY_TEST_EXIT_CODE", "unknown"))
          print('<?xml version="1.0" encoding="UTF-8"?>')
          print('<testsuite name="nextest-preflight" tests="1" failures="1">')
          print('<testcase classname="ci" name="missing-nextest-junit">')
          print(f'<failure message="nextest JUnit report was not produced">MERGIFY_TEST_EXIT_CODE={rc}; see the Run tests log.</failure>')
          print('</testcase></testsuite>')
          PY
          fi"""
    good_full_workflow = good_full_workflow.replace(root_stage_step, root_stage_step_with_fallback).replace(
        bvs_stage_step, bvs_stage_step_with_fallback
    )
    good_smoke_workflow = good_smoke_workflow.replace(root_stage_step, root_stage_step_with_fallback).replace(
        bvs_stage_step, bvs_stage_step_with_fallback
    )
    smoke_bvs_stage_step = bvs_stage_step_with_fallback
    good_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    flaky_errors = [error for error in good_errors if "flaky-test-detection" in error]
    if flaky_errors:
        raise AssertionError(f"flaky detection workflow verifier must accept split workflows, got: {flaky_errors}")
    expected_partitioned_bvs_policy_labels = frozenset({"backtester full job", "backtester smoke job"})
    if verifier.PARTITIONED_BVS_BACKTESTER_POLICY_LABELS != expected_partitioned_bvs_policy_labels:
        raise AssertionError(
            "partitioned BVS step allowlist must stay scoped to the sharded backtester full/smoke jobs"
        )

    echo_exit_full_workflow = good_full_workflow.replace(
        '          exit "$rc"',
        '          echo \'exit "$rc"\'',
        1,
    )
    echo_exit_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: echo_exit_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("root full job missing 'exit \"$rc\"'" in error for error in echo_exit_errors):
        raise AssertionError(
            "flaky detection verifier must reject echo-spoofed exit propagation, "
            f"got: {echo_exit_errors}"
        )

    stage_if_spoof_full_workflow = good_full_workflow.replace(
        """      - name: Stage JUnit report
        if: success() || failure()
""",
        """      - name: Stage JUnit report
        env:
          SPOOF: "if: success() || failure()"
""",
        1,
    )
    stage_if_spoof_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: stage_if_spoof_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("root full job missing 'if: success() || failure()'" in error for error in stage_if_spoof_errors):
        raise AssertionError(
            "flaky detection verifier must reject non-step-level stage if spoofing, "
            f"got: {stage_if_spoof_errors}"
        )

    stage_junit_echo_full_workflow = good_full_workflow.replace(
        '          python3 - > "$staged" <<\'PY\'',
        '          echo missing-nextest-junit > "$staged"',
        1,
    )
    stage_junit_echo_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: stage_junit_echo_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("root full job JUnit staging must synthesize a missing-report failure" in error for error in stage_junit_echo_errors):
        raise AssertionError(
            "flaky detection verifier must reject synthetic-JUnit echo spoofing, "
            f"got: {stage_junit_echo_errors}"
        )

    def flaky_detection_errors(
        full_workflow: str = good_full_workflow,
        smoke_workflow: str = good_smoke_workflow,
    ) -> list[str]:
        return [
            error
            for error in verifier.verify_flaky_test_detection_workflows(
                {
                    full_workflow_name: full_workflow,
                    smoke_workflow_name: smoke_workflow,
                }
            )
            if "flaky-test-detection" in error
        ]

    def has_bvs_step_allowlist_error(errors: list[str]) -> bool:
        return any("backtester smoke job must keep BVS job steps unchanged" in error for error in errors)

    resharded_full_workflow = good_full_workflow.replace(
        "shard: [1, 2, 3, 4]",
        "shard: [1, 2, 3, 4, 5]",
    ).replace('partition "count:${{ matrix.shard }}/4"', 'partition "count:${{ matrix.shard }}/5"')
    resharded_full_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: resharded_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    resharded_full_flaky_errors = [error for error in resharded_full_errors if "flaky-test-detection" in error]
    if resharded_full_flaky_errors:
        raise AssertionError(
            f"flaky detection verifier must accept manual full workflow shard-count changes, got: {resharded_full_flaky_errors}"
        )
    rerun_full_workflow = good_full_workflow.replace(
        "run_number: [1, 2, 3, 4, 5]",
        "run_number: [1, 2, 3, 4, 5, 6]",
    )
    rerun_full_errors = flaky_detection_errors(full_workflow=rerun_full_workflow)
    if rerun_full_errors:
        raise AssertionError(
            f"flaky detection verifier must accept manual full workflow run-count changes, got: {rerun_full_errors}"
        )
    noncontiguous_run_number_full_errors = flaky_detection_errors(
        full_workflow=good_full_workflow.replace("run_number: [1, 2, 3, 4, 5]", "run_number: [1, 3, 4]")
    )
    if not any("full job run_number matrix must be one-indexed and contiguous" in error for error in noncontiguous_run_number_full_errors):
        raise AssertionError(
            "flaky detection verifier must reject non-contiguous full workflow run numbers, "
            f"got: {noncontiguous_run_number_full_errors}"
        )
    mismatched_reshard_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: resharded_full_workflow.replace(
                'partition "count:${{ matrix.shard }}/5"',
                'partition "count:${{ matrix.shard }}/4"',
            ),
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job partition denominator must match shard matrix length" in error for error in mismatched_reshard_errors):
        raise AssertionError(
            "flaky detection verifier must reject full workflow shard/partition denominator drift, "
            f"got: {mismatched_reshard_errors}"
        )
    missing_partition_full_workflow = good_full_workflow.replace(
        '--partition "count:${{ matrix.shard }}/4" ',
        "",
        1,
    ).replace(
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip',
        'echo "count:${{ matrix.shard }}/4"\n          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip',
        1,
    )
    missing_partition_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: missing_partition_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job must have one matrix.shard partition argument" in error for error in missing_partition_errors):
        raise AssertionError(
            "flaky detection verifier must reject denominator text outside the partition argument, "
            f"got: {missing_partition_errors}"
        )
    spoofed_partition_full_workflow = good_full_workflow.replace(
        '--partition "count:${{ matrix.shard }}/4" ',
        "",
        1,
    ).replace(
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip',
        'echo \'--partition "count:${{ matrix.shard }}/4"\'\n          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip',
        1,
    )
    spoofed_partition_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: spoofed_partition_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job must have one matrix.shard partition argument" in error for error in spoofed_partition_errors):
        raise AssertionError(
            "flaky detection verifier must reject partition text outside the just bte-test argument, "
            f"got: {spoofed_partition_errors}"
        )
    dead_partition_full_workflow = good_full_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          if false; then
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          fi
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl""",
        1,
    )
    dead_partition_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: dead_partition_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job must have exactly one just bte-test invocation" in error for error in dead_partition_errors):
        raise AssertionError(
            "flaky detection verifier must reject dead-code partition lines plus live unpartitioned test commands, "
            f"got: {dead_partition_errors}"
        )
    chained_partition_full_workflow = good_full_workflow.replace(
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl; just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl',
        1,
    )
    chained_partition_full_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: chained_partition_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job must have exactly one just bte-test invocation" in error for error in chained_partition_full_errors):
        raise AssertionError(
            "flaky detection verifier must reject same-line full BVS extra test invocations, "
            f"got: {chained_partition_full_errors}"
        )
    for operator in (";", "&&", "||", "|"):
        chained_shell_full_workflow = good_full_workflow.replace(
            'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
            'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl '
            + operator
            + " echo after",
            1,
        )
        chained_shell_full_errors = flaky_detection_errors(full_workflow=chained_shell_full_workflow)
        if not any("backtester full job must keep just bte-test in a simple Run tests block" in error for error in chained_shell_full_errors):
            raise AssertionError(
                f"flaky detection verifier must reject full BVS shell chaining with {operator!r}, "
                f"got: {chained_shell_full_errors}"
            )
        no_space_chained_shell_full_workflow = good_full_workflow.replace(
            'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
            'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl'
            + operator
            + "echo after",
            1,
        )
        no_space_chained_shell_full_errors = flaky_detection_errors(full_workflow=no_space_chained_shell_full_workflow)
        if not any(
            "backtester full job must keep just bte-test in a simple Run tests block" in error
            for error in no_space_chained_shell_full_errors
        ):
            raise AssertionError(
                f"flaky detection verifier must reject full BVS shell chaining without spaces around {operator!r}, "
                f"got: {no_space_chained_shell_full_errors}"
            )
    subshell_partition_full_workflow = good_full_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          ( just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl
          )
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl""",
        1,
    )
    subshell_partition_full_errors = flaky_detection_errors(full_workflow=subshell_partition_full_workflow)
    if not any("backtester full job must have exactly one just bte-test invocation" in error for error in subshell_partition_full_errors):
        raise AssertionError(
            "flaky detection verifier must reject subshell full BVS extra test invocations, "
            f"got: {subshell_partition_full_errors}"
        )
    wrapped_subshell_full_workflow = good_full_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          (
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          )""",
        1,
    )
    wrapped_subshell_full_errors = flaky_detection_errors(full_workflow=wrapped_subshell_full_workflow)
    if not any("backtester full job must keep just bte-test in a simple Run tests block" in error for error in wrapped_subshell_full_errors):
        raise AssertionError(
            "flaky detection verifier must reject multiline subshell-wrapped full BVS commands, "
            f"got: {wrapped_subshell_full_errors}"
        )
    for wrapped_command, wrapper_name in (
        (
            """          echo "preflight"
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl""",
            "extra shell statement",
        ),
        (
            """          ignored=`
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          `""",
            "backtick command-substitution",
        ),
        (
            """          {
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          }""",
            "brace-group",
        ),
        (
            """          cat <(
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          )""",
            "process-substitution",
        ),
        (
            """          cat <<'EOF'
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          EOF""",
            "heredoc",
        ),
        (
            """          ignored='
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          '""",
            "multiline quoted string",
        ),
        (
            """          bvs_words=(
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          )""",
            "array assignment",
        ),
        (
            """          run_bte() {
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          }""",
            "dead function definition",
        ),
        (
            """          run_bte() {
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          }
          run_bte""",
            "called function definition",
        ),
        (
            """          function run_bte {
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          }""",
            "dead function keyword definition",
        ),
        (
            """          function run_bte () {
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          }
          run_bte
          run_bte""",
            "called function keyword definition",
        ),
    ):
        wrapped_full_workflow = good_full_workflow.replace(
            '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
            wrapped_command,
            1,
        )
        wrapped_full_errors = flaky_detection_errors(full_workflow=wrapped_full_workflow)
        if not any("backtester full job must keep just bte-test in a simple Run tests block" in error for error in wrapped_full_errors):
            raise AssertionError(
                f"flaky detection verifier must reject {wrapper_name} full BVS commands, "
                f"got: {wrapped_full_errors}"
            )
    dead_only_partition_full_workflow = good_full_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          if false; then
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          fi""",
        1,
    )
    dead_only_partition_full_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: dead_only_partition_full_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("backtester full job must keep just bte-test in a simple Run tests block" in error for error in dead_only_partition_full_errors):
        raise AssertionError(
            "flaky detection verifier must reject dead-only full BVS test invocations, "
            f"got: {dead_only_partition_full_errors}"
        )
    for guarded_command in (
        """          if ! true; then
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          fi""",
        """          while false; do
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          done""",
        """          until true; do
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          done""",
    ):
        guarded_full_workflow = good_full_workflow.replace(
            '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
            guarded_command,
            1,
        )
        guarded_full_errors = flaky_detection_errors(full_workflow=guarded_full_workflow)
        if not any("backtester full job must keep just bte-test in a simple Run tests block" in error for error in guarded_full_errors):
            raise AssertionError(
                "flaky detection verifier must reject guarded full BVS test invocations, "
                f"got: {guarded_full_errors}"
            )

    missing_smoke_partition_workflow = good_smoke_workflow.replace(
        '--partition "count:${{ matrix.shard }}/4" ',
        "",
        1,
    )
    missing_smoke_partition_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: missing_smoke_partition_workflow,
        }
    )
    if not any("backtester smoke job must have one matrix.shard partition argument" in error for error in missing_smoke_partition_errors):
        raise AssertionError(
            "flaky detection verifier must reject scheduled smoke without a partitioned BVS shard, "
            f"got: {missing_smoke_partition_errors}"
        )
    chained_partition_smoke_workflow = good_smoke_workflow.replace(
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl; just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl',
        1,
    )
    chained_partition_smoke_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: chained_partition_smoke_workflow,
        }
    )
    if not any("backtester smoke job must have exactly one just bte-test invocation" in error for error in chained_partition_smoke_errors):
        raise AssertionError(
            "flaky detection verifier must reject same-line scheduled smoke extra test invocations, "
            f"got: {chained_partition_smoke_errors}"
        )
    command_substitution_smoke_workflow = good_smoke_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          ignored="$(just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl)"
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl""",
        1,
    )
    command_substitution_smoke_errors = flaky_detection_errors(smoke_workflow=command_substitution_smoke_workflow)
    if not any("backtester smoke job must have exactly one just bte-test invocation" in error for error in command_substitution_smoke_errors):
        raise AssertionError(
            "flaky detection verifier must reject command-substitution scheduled smoke extra test invocations, "
            f"got: {command_substitution_smoke_errors}"
        )
    multiline_command_substitution_smoke_workflow = good_smoke_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          ignored="$(
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          )" """,
        1,
    )
    multiline_command_substitution_smoke_errors = flaky_detection_errors(
        smoke_workflow=multiline_command_substitution_smoke_workflow
    )
    if not any(
        "backtester smoke job must keep just bte-test in a simple Run tests block" in error
        for error in multiline_command_substitution_smoke_errors
    ):
        raise AssertionError(
            "flaky detection verifier must reject multiline command-substitution scheduled smoke BVS commands, "
            f"got: {multiline_command_substitution_smoke_errors}"
        )
    dead_only_partition_smoke_workflow = good_smoke_workflow.replace(
        '          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl',
        """          if false; then
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl
          fi""",
        1,
    )
    dead_only_partition_smoke_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: dead_only_partition_smoke_workflow,
        }
    )
    if not any("backtester smoke job must keep just bte-test in a simple Run tests block" in error for error in dead_only_partition_smoke_errors):
        raise AssertionError(
            "flaky detection verifier must reject dead-only scheduled smoke test invocations, "
            f"got: {dead_only_partition_smoke_errors}"
        )
    changed_stage_shell_smoke_workflow = good_smoke_workflow.replace(
        smoke_bvs_stage_step,
        """        run: |
          if false; then
          echo "skip an unrelated staging branch"
          fi
          cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml" """,
        1,
    )
    changed_stage_shell_smoke_errors = flaky_detection_errors(smoke_workflow=changed_stage_shell_smoke_workflow)
    if not has_bvs_step_allowlist_error(changed_stage_shell_smoke_errors):
        raise AssertionError(
            "flaky detection verifier must reject changed sibling shell steps in the scheduled smoke BVS job, "
            f"got: {changed_stage_shell_smoke_errors}"
        )
    cross_step_bte_smoke_workflow = good_smoke_workflow.replace(
        smoke_bvs_stage_step,
        """        run: |
          just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl
          cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml" """,
        1,
    )
    cross_step_bte_smoke_errors = flaky_detection_errors(smoke_workflow=cross_step_bte_smoke_workflow)
    if not any("backtester smoke job must have exactly one just bte-test invocation" in error for error in cross_step_bte_smoke_errors):
        raise AssertionError(
            "flaky detection verifier must reject scheduled smoke BVS commands outside the Run tests step, "
            f"got: {cross_step_bte_smoke_errors}"
        )
    for extra_command, extra_command_name in (
        (
            'just  bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" -- --skip issue_789_first_real_free_data_taker_pl',
            "whitespace-obfuscated just bte-test",
        ),
        (
            'cargo nextest run -p backtesting-vertical-slice',
            "raw cargo nextest BVS",
        ),
    ):
        cross_step_extra_work_smoke_workflow = good_smoke_workflow.replace(
            smoke_bvs_stage_step,
            f"""        run: |
          {extra_command}
          cp "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml" "junit-unit-${{ matrix.run_number }}.xml" """,
            1,
        )
        cross_step_extra_work_smoke_errors = flaky_detection_errors(smoke_workflow=cross_step_extra_work_smoke_workflow)
        if not (
            has_bvs_step_allowlist_error(cross_step_extra_work_smoke_errors)
            or any(
                "backtester smoke job must have exactly one just bte-test invocation" in error
                for error in cross_step_extra_work_smoke_errors
            )
        ):
            raise AssertionError(
                f"flaky detection verifier must reject {extra_command_name} commands outside the Run tests step, "
                f"got: {cross_step_extra_work_smoke_errors}"
            )
    for extra_step, extra_step_name in (
        (
            """      - run: cargo nextest run -p backtesting-vertical-slice
""",
            "nameless run step",
        ),
        (
            """      - id: warm_bvs
        run: cargo nextest run -p backtesting-vertical-slice
""",
            "id-led run step",
        ),
        (
            """      - uses: ./run-bvs-tests
""",
            "unexpected uses step",
        ),
        (
            """      - name: Run tests
        run: cargo nextest run -p backtesting-vertical-slice
""",
            "duplicate Run tests step",
        ),
    ):
        extra_step_smoke_workflow = good_smoke_workflow.replace(
            '      - name: Stage JUnit report\n'
            + smoke_bvs_stage_step,
            extra_step
            + '      - name: Stage JUnit report\n'
            + smoke_bvs_stage_step,
            1,
        )
        extra_step_smoke_errors = flaky_detection_errors(smoke_workflow=extra_step_smoke_workflow)
        if not has_bvs_step_allowlist_error(extra_step_smoke_errors):
            raise AssertionError(
                f"flaky detection verifier must reject {extra_step_name} in the scheduled smoke BVS job, "
                f"got: {extra_step_smoke_errors}"
            )

    oversized_smoke_workflow = good_smoke_workflow.replace(
        "run_number: [1]",
        "run_number: [1, 2, 3, 4, 5]",
    )
    oversized_smoke_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: oversized_smoke_workflow,
        }
    )
    if not any("root smoke job" in error and "run_number: [1]" in error for error in oversized_smoke_errors):
        raise AssertionError(
            f"flaky detection verifier must reject multi-run smoke root jobs, got: {oversized_smoke_errors}"
        )
    incomplete_smoke_workflow = good_smoke_workflow.replace("shard: [1, 2, 3, 4]", "shard: [1]")
    incomplete_smoke_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: incomplete_smoke_workflow,
        }
    )
    if not any(
        "backtester smoke job partition denominator must match shard matrix length" in error
        for error in incomplete_smoke_errors
    ):
        raise AssertionError(
            "flaky detection verifier must reject incomplete smoke BVS shard coverage, "
            f"got: {incomplete_smoke_errors}"
        )

    missing_smoke_workflow = good_smoke_workflow.replace("  flaky-smoke-rust-backtester:\n", "  removed-backtester-smoke:\n")
    missing_smoke_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
            smoke_workflow_name: missing_smoke_workflow,
        }
    )
    if not any("backtester smoke job" in error for error in missing_smoke_errors):
        raise AssertionError(f"flaky detection verifier must reject missing BVS smoke job, got: {missing_smoke_errors}")

    missing_workflow_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: good_full_workflow,
        }
    )
    expected_missing_workflow_errors = [
        f"{smoke_workflow_name}: flaky-test-detection required workflow is missing",
    ]
    assert missing_workflow_errors == expected_missing_workflow_errors, (
        f"flaky detection verifier must report missing workflow files, got: {missing_workflow_errors}"
    )

    drift_cases = (
        (
            "root full job missing 'exit \"$rc\"'",
            good_full_workflow.replace("          exit \"$rc\"\n", "", 1),
            good_smoke_workflow,
        ),
        (
            "root full job missing 'if: success() || failure()'",
            good_full_workflow.replace("        if: success() || failure()\n", "", 1),
            good_smoke_workflow,
        ),
        (
            "workflow triggers must be ['workflow_dispatch']",
            good_full_workflow.replace(
                "on:\n  workflow_dispatch:\n",
                "on:\n  workflow_dispatch:\n  schedule:\n    - cron: '0 */12 * * 1-5'\n",
                1,
            ),
            good_smoke_workflow,
        ),
        (
            "workflow triggers must be ['schedule', 'workflow_dispatch']",
            good_full_workflow,
            good_smoke_workflow.replace("on:\n  workflow_dispatch:\n  schedule:\n    - cron: '0 */12 * * 1-5'\n", "on:\n  schedule:\n    - cron: '0 */12 * * 1-5'\n  push:\n", 1),
        ),
        (
            "root smoke job missing 'if: success() || failure()'",
            good_full_workflow,
            good_smoke_workflow.replace("        if: success() || failure()\n", "", 1),
        ),
        (
            "root smoke job missing 'set +e'",
            good_full_workflow,
            good_smoke_workflow.replace("          set +e\n", "", 1),
        ),
    )
    for expected, full_workflow, smoke_workflow in drift_cases:
        drift_errors = verifier.verify_flaky_test_detection_workflows(
            {
                full_workflow_name: full_workflow,
                smoke_workflow_name: smoke_workflow,
            }
        )
        if not any(expected in error for error in drift_errors):
            raise AssertionError(f"flaky detection verifier must reject {expected!r}, got: {drift_errors}")

    managed_target_workflow = good_full_workflow.replace(
        'report="target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"',
        'report="${{ steps.setup.outputs.managed_target_dir }}/nextest/default/junit-unit-${{ matrix.run_number }}.xml"',
        1,
    ).replace(
        'report="crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"',
        'report="${{ steps.crate_target.outputs.dir }}/nextest/default/junit-unit-${{ matrix.run_number }}.xml"',
        1,
    )
    managed_target_errors = verifier.verify_flaky_test_detection_workflows(
        {
            full_workflow_name: managed_target_workflow,
            smoke_workflow_name: good_smoke_workflow,
        }
    )
    if not any("root JUnit staging" in error for error in managed_target_errors):
        raise AssertionError(f"flaky detection verifier must reject root managed-target staging, got: {managed_target_errors}")
    if not any("backtester JUnit staging" in error for error in managed_target_errors):
        raise AssertionError(f"flaky detection verifier must reject BVS managed-target staging, got: {managed_target_errors}")


def assert_ra001a_dispatch_cancel_guard_is_fail_closed() -> None:
    verifier = load_verifier()
    guarded_cancel = """cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        || (github.event_name == 'workflow_dispatch'
            && inputs.ra001a_durable_tracer != true) }}"""
    if not verifier.cancel_in_progress_is_merge_group_safe(guarded_cancel):
        raise AssertionError(
            "the RA-001a dispatch opt-out is event-guarded and must preserve merge_group runs"
        )

    unsafe_mutations = {
        "missing dispatch event guard": guarded_cancel.replace(
            "(github.event_name == 'workflow_dispatch'\n            && ", "(", 1
        ),
        "or instead of and": guarded_cancel.replace(
            "github.event_name == 'workflow_dispatch'\n            &&",
            "github.event_name == 'workflow_dispatch'\n            ||",
            1,
        ),
        "merge-group event arm": guarded_cancel.replace(
            "github.event_name == 'workflow_dispatch'",
            "github.event_name == 'merge_group'",
            1,
        ),
    }
    for label, mutated in unsafe_mutations.items():
        if verifier.cancel_in_progress_is_merge_group_safe(mutated):
            raise AssertionError(f"cancel guard must reject {label}")


def main() -> int:
    assert_ra001a_dispatch_cancel_guard_is_fail_closed()
    assert_merge_group_support_gaps_are_reported()
    assert_gate_policy_truth_table_gaps_are_reported()
    assert_flaky_detection_workflows_are_split_without_mode_gates()
    assert_flaky_detection_workflow_split_gaps_are_reported()
    print("OK: workflow expression analysis tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
