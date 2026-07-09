#!/usr/bin/env python3
"""Relocated CI workflow hygiene analyzer tests."""

from __future__ import annotations

import sys
import textwrap

from ci_workflow_hygiene_test_helpers import (
    BASE_ACTION,
    BASE_ADVISORY_WORKFLOW,
    BASE_NEXTEST_CONFIG,
    BASE_WORKFLOW,
    load_verifier,
    workflow_with_detector_probe,
)

def assert_v6_deploy_artifact_s3_stays_allowed() -> None:
    verifier = load_verifier()
    workflow = workflow_with_detector_probe(
        """
        mkdir -p dist
        aws s3 cp dist/bolt-v2.tar.zst s3://bolt-v2-deploy-artifacts/bolt-v2.tar.zst
        aws s3 cp "$PWD/dist/bolt-v2.sha256" s3://bolt-v2-deploy-artifacts/bolt-v2.sha256
        """
    )
    s3_errors = [error for error in verifier.verify_text(workflow, BASE_ACTION, BASE_NEXTEST_CONFIG) if "s3" in error.lower()]
    if s3_errors:
        raise AssertionError(f"deploy artifact S3 publication must stay allowed, got: {s3_errors!r}")

def assert_v6_red_s3_storage_transfer_policy_is_semantic() -> None:
    verifier = load_verifier()
    expected = "S3 active mutable target cache must be rejected"
    workflows = {
        "s3 destination hidden behind an env value": """
            DEST=s3://bolt-v2-active-cache/workspace
            aws s3 sync "$GITHUB_WORKSPACE" "$DEST"
        """,
        "workspace source hidden behind an env value": """
            SRC=$PWD
            aws s3 sync "$SRC" s3://bolt-v2-active-cache/workspace
        """,
        "managed target hidden behind command substitution": """
            TARGET=`python3 scripts/rust_verification.py target-dir --repo .`
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "inline backtick target operand": """
            aws s3 sync `echo target` s3://bolt-v2-active-cache/target
        """,
        "aws global option before active target endpoints": """
            aws s3 sync --endpoint-url https://example.com target s3://bolt-v2-active-cache/target
        """,
        "workspace root dot path": """
            aws s3 sync "$PWD/." s3://bolt-v2-active-cache/workspace
        """,
        "github workspace root dot path": """
            aws s3 sync "$GITHUB_WORKSPACE/." s3://bolt-v2-active-cache/workspace
        """,
        "github env unquoted assignment": """
            echo TARGET=target >> $GITHUB_ENV
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "github env quoted value assignment": """
            echo 'TARGET="target"' >> "$GITHUB_ENV"
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "s3 destination hidden behind printf github env": """
            printf 'DEST=%s\\n' "s3://bolt-v2-active-cache/target" >> "$GITHUB_ENV"
            aws s3 sync target "$DEST"
        """,
        "active target hidden behind second printf github env assignment": """
            printf 'BENIGN=dist\\nSRC=target\\n' >> "$GITHUB_ENV"
            aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
        """,
        "active target copied through neutral staging path": """
            mkdir /tmp/deploy
            cp -r target/debug /tmp/deploy/
            aws s3 sync /tmp/deploy s3://bolt-v2-active-cache/cache
        """,
        "active target cwd hidden behind env chdir wrapper": """
            env -C target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind sudo chdir wrapper": """
            sudo --chdir target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind cd separator": """
            cd -- target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind cd option and separator": """
            cd -L -- target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind combined cd options": """
            cd -LP target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target streamed through s3 stdin": """
            tar -czf - target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through cat to s3 stdin": """
            cat target/debug/libbolt_v2.rmeta | aws s3 cp - s3://bolt-v2-active-cache/cache
        """,
        "active target streamed through clustered tar stdout flag": """
            tar -czf- target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through traditional tar stdout flag": """
            tar cf - target | aws s3 cp - s3://bolt-v2-active-cache/target.tar
        """,
        "active target streamed through tar long file stdout flag": """
            tar -c --file=- target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through tar default stdout": """
            tar c target | aws s3 cp - s3://bolt-v2-active-cache/target.tar
        """,
        "active target streamed through fused input redirection": """
            cat <target/debug/libbolt_v2.rmeta | aws s3 cp - s3://bolt-v2-active-cache/cache
        """,
        "s3 stdout written to active target through shell group redirection": """
            { aws s3 cp s3://bolt-v2-active-cache/target.tar - ; } > target/cache.tar
        """,
        "s3 stdout written to active target through subshell redirection": """
            ( aws s3 cp s3://bolt-v2-active-cache/target.tar - ) > target/cache.tar
        """,
        "active target moved through local staging before S3 upload": """
            mv target my_cache
            aws s3 cp my_cache s3://bolt-v2-active-cache/target.tar
        """,
        "s3 download moved from local staging into active target": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar my_cache
            mv my_cache target
        """,
        "active target archived locally before S3 upload": """
            tar -cf cache.tar target
            aws s3 cp cache.tar s3://bolt-v2-active-cache/target.tar
        """,
        "active target zipped locally before S3 upload": """
            zip -r cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with option argument before S3 upload": """
            zip -b /tmp cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with clustered option argument before S3 upload": """
            zip -qr0b /tmp cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with exclude option argument before S3 upload": """
            zip -x ignored cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "s3 archive downloaded locally before active target extraction": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar -xf cache.tar -C target
        """,
        "s3 archive extraction names active target as operand": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar xf cache.tar target
        """,
        "s3 zip downloaded locally before active target extraction": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip cache.zip -d target
        """,
        "s3 zip extraction names active target as operand": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip cache.zip target/*
        """,
        "s3 zip extraction skips option argument before archive": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip -x ignored cache.zip -d target
        """,
        "s3 zip extraction handles clustered directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip -qd target cache.zip
        """,
        "s3 download moved to active target through mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -t target s3_cache
        """,
        "s3 download moved to active target through clustered mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -vt target s3_cache
        """,
        "s3 download moved to active target through concatenated mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -ttarget s3_cache
        """,
        "s3 download copied to active target through cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp --target-directory=target s3_cache
        """,
        "s3 download copied to active target through clustered cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp -vt target s3_cache
        """,
        "s3 download copied to active target through concatenated cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp -ttarget s3_cache
        """,
        "s3 tar extraction handles ordered traditional options": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar xCf target cache.tar
        """,
        "s3 tar extraction handles ordered clustered options": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar -xCf target cache.tar
        """,
        "s3 transfer hidden behind su command string": """
            su -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind su long command string": """
            su --command="aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind su clustered command string": """
            su -mc "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind sg command string": """
            sg docker -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind sg concatenated command string": """
            sg docker -c"aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind runuser long command string": """
            runuser --command="aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind runuser user command string": """
            runuser user -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind flock clustered command string": """
            flock /tmp/lock -c"aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "env chdir ignores C inside unset option argument": """
            env -uC -C target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "sudo chdir ignores D inside user option argument": """
            sudo -uD -D target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "s3 transfer hidden behind rustup shell command string": """
            rustup run nightly sh -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
    }
    misses: list[str] = []
    for name, script in workflows.items():
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(script))
        if expected not in errors:
            misses.append(f"{name}: {errors!r}")
    if misses:
        raise AssertionError("storage-transfer policy did not classify semantic active-cache flows: " + "; ".join(misses))

def assert_v6_workflow_run_steps_reset_shell_state() -> None:
    verifier = load_verifier()
    s3_expected = "S3 active mutable target cache must be rejected"
    target_expected = "CARGO_TARGET_DIR raw target override must be classified"
    cases = [
        (
            "same run step cwd must classify active-target S3",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            True,
        ),
        (
            "same run step cd without target must clear active-target cwd",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      cd
                      aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "same run step cd separator without target must clear active-target cwd",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      cd --
                      aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "separate run step cwd must not leak into later S3 step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                  - run: |
                      aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            False,
        ),
        (
            "flexibly indented same run step cwd must classify active-target S3",
            """
            jobs:
              test:
                steps:
                    - run: |
                        cd target
                        aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            True,
        ),
        (
            "flexibly indented separate run step cwd must not leak into later S3 step",
            """
            jobs:
              test:
                steps:
                    - run: |
                        cd target
                    - run: |
                        aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            False,
        ),
        (
            "same run step target env alias must classify raw target override",
            """
            jobs:
              test:
                steps:
                  - run: |
                      E=CARGO_TARGET_DIR
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "separate run step target env alias must not leak into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      E=CARGO_TARGET_DIR
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            False,
        ),
        (
            "github env target must persist into later S3 step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo TARGET=target >> $GITHUB_ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env target must persist after earlier command in same step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      mkdir -p out && echo TARGET=target >> $GITHUB_ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env continued active target must persist into later step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "TARGET=target" \\
                        >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env target must not persist across jobs",
            """
            jobs:
              producer:
                steps:
                  - run: |
                      echo TARGET=target >> $GITHUB_ENV
              consumer:
                steps:
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            False,
        ),
        (
            "github env target key alias must persist into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo E=CARGO_TARGET_DIR >> $GITHUB_ENV
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env echo flags must persist target key alias into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo -e "E=CARGO_TARGET_DIR" >> $GITHUB_ENV
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env printf must persist target key alias into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'E=%s\\n' CARGO_TARGET_DIR >> "$GITHUB_ENV"
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env printf multiple assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'SRC=target\\nDEST=s3://bolt-v2-active-cache/cache\\n' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf repeated format assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf '%s\\n' BENIGN=1 SRC=target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf missing argument still persists prior assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=%s\\nEXTRA=%s\\n' target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf missing argument clears stale assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "SRC=target" >> "$GITHUB_ENV"
                  - run: |
                      printf 'SRC=%s\\n' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env printf literal format persists despite extra argument",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=target\\n' benign >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf b conversion assignment must persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=%b\\n' target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf b conversion decodes argument newlines",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf '%b' 'SRC=target\\nDEST=s3://bolt-v2-active-cache/cache' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf escaped percent must not consume argument",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'A=%%s\\n' SRC=target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env printf arguments must not decode escaped newlines",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'A=%s\\n' 'B=1\\nSRC=target' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env echo e multiple assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo -e "SRC=target\\nDEST=s3://bolt-v2-active-cache/cache" >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<'ENV'
                      SRC<<EOF
                      target
                      EOF
                      DEST=s3://bolt-v2-active-cache/cache
                      ENV
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc must overwrite earlier inline assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "SRC=benign" >> "$GITHUB_ENV"
                      cat >> "$GITHUB_ENV" <<ENV
                      SRC=target
                      ENV
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc delimiter must be exact and preserve continuation",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<ENV
                      BENIGN=1
                       ENV
                      TARGET=targ\\
                      et
                      ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "quoted shell heredoc delimiter must not fold escaped newline payload",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<'ENV'
                      TARGET=targ\\
                      et
                      ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "separate composite action run step cwd must not leak into later S3 step",
            """
            runs:
              using: composite
              steps:
                - shell: bash
                  run: cd target
                - shell: bash
                  run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "quoted composite action using value must still isolate step cwd",
            """
            runs:
              using: "composite"
              steps:
                - shell: bash
                  run: cd target
                - shell: bash
                  run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "indentless composite action run step cwd must not leak into later S3 step",
            """
            runs:
              using: composite
              steps:
              - shell: bash
                run: cd target
              - shell: bash
                run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "indentless github env target must persist into later S3 step",
            """
            jobs:
              test:
                steps:
                - run: |
                    echo TARGET=target >> $GITHUB_ENV
                - run: |
                    aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
    ]
    failures: list[str] = []
    for name, workflow_text, expected, should_find in cases:
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow_text))
        found = expected in errors
        if found != should_find:
            failures.append(f"{name}: expected found={should_find}, got {errors!r}")
    env_assignment = verifier.github_env_assignment_line('mkdir -p out && echo "TARGET=target dir" >> "$GITHUB_ENV"')
    if env_assignment != "TARGET='target dir'":
        failures.append(f"GITHUB_ENV assignment with spaces was not shell-safe: {env_assignment!r}")
    if failures:
        raise AssertionError("workflow run step shell state handling failed: " + "; ".join(failures))

def assert_v6_red_raw_rust_storage_overrides_are_reported() -> None:
    cases = [
        (
            "CARGO_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; env $E=/tmp/raw-target cargo test",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'E=CARGO_TARGET_DIR; C=cargo; env -S "env $E=/tmp/raw-target $C check"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'env -iS "CARGO_TARGET_DIR=/tmp/raw-target cargo test"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'E=CARGO_TARGET_DIR; env -iS "$E=/tmp/raw-target cargo test"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR\nexport $E=/tmp/raw-target\ncargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; eval \"export $E=/tmp/raw-target\"; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; $E=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO; ${E}_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "alias c='V=CARGO; eval \"${V}_TARGET_DIR=/tmp/raw cargo\"'; c build",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; bash -c \"${V}_TARGET_DIR=/tmp/raw cargo test\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; sudo ${V}_TARGET_DIR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; time ${V}_TARGET_DIR=/tmp/raw cargo test",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; CMD=\"${V}_TARGET_DIR=/tmp/raw cargo check\"; eval \"$CMD\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; export CMD=\"${V}_TARGET_DIR=/tmp/raw cargo check\"; bash -c \"$CMD\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; alias c='${V}_TARGET_DIR=/tmp/raw cargo'; c build",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "CMD=\"cargo check --target-dir /tmp/raw\"; eval \"$CMD\"",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo>out check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "> /dev/null cargo check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "ARGS=\"--target-dir /tmp/raw\"; cargo check $ARGS",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "echo 'benign'\nE=CARGO_TARGET_DIR\nenv $E=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "run: |\n  C=cargo\n  $C check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'VAR=CARGO; eval "${VAR}_TARGET_DIR=/tmp/raw cargo check"; VAR=BENIGN',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=$(echo CARGO_TARGET_DIR); export $E=/tmp/raw-target; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "export E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "declare -x E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_TARGET_DIR: /tmp/raw",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_TARGET_DIR\": /tmp/raw",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "CARGO_BUILD_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_BUILD_TARGET_DIR: /tmp/raw",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_BUILD_TARGET_DIR\": /tmp/raw",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "mkdir -p .cargo && printf '[build]\\ntarget-dir = \"/tmp/raw-target\"\\n' > .cargo/config.toml && cargo check",
            ".cargo/config.toml build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build.target-dir=/tmp/raw-target' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { target-dir = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { \"target\\u002Ddir\" = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { \"target\\U0000002Ddir\" = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build.rustflags = [\"--out-dir\", \"/tmp/raw-out\"]' check",
            "cargo --config build.rustflags raw output override must be classified",
        ),
        (
            "cargo --config 'build = { rustflags = [\"--artifact-dir\", \"/tmp/raw-artifacts\"] }' check",
            "cargo --config build.rustflags raw output override must be classified",
        ),
        (
            "run: |\n  cargo check \\\n    --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "run: >\n  cargo check\n  --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo check --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "python -c \"import os; os.system('c' + 'argo build --target-dir /tmp/raw')\"",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'python -c "import subprocess; subprocess.run([\'cargo\', \'build\', \'--target-dir\', \'/tmp/raw\'])"',
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'python -c "import subprocess; subprocess.run(args=[\'cargo\', \'build\', \'--target-dir\', \'/tmp/raw\'])"',
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "echo 'cargo \"$@\"' | bash -s -- build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "CARGO_TARGET_TMPDIR=/tmp/raw-tmp cargo test",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_TARGET_TMPDIR: /tmp/raw-tmp",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_TARGET_TMPDIR\": /tmp/raw-tmp",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "CARGO_INCREMENTAL=1 cargo check",
            "CARGO_INCREMENTAL raw cache override must be classified",
        ),
        (
            "CARGO_ENCODED_RUSTFLAGS='--out-dir\\x1f/tmp/raw-out' cargo check",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "CARGO_BUILD_RUSTFLAGS='--out-dir /tmp/raw-out' cargo check",
            "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  CARGO_BUILD_RUSTFLAGS: '--artifact-dir /tmp/raw-artifacts'",
            "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  CARGO_ENCODED_RUSTFLAGS: '--out-dir\\x1f/tmp/raw-out'",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  \"CARGO_ENCODED_RUSTFLAGS\": '--out-dir\\x1f/tmp/raw-out'",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "CARGO_INSTALL_ROOT=/tmp/cargo-install cargo install ripgrep --locked",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "env:\n  CARGO_INSTALL_ROOT: /tmp/cargo-install",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "env:\n  \"CARGO_INSTALL_ROOT\": /tmp/cargo-install",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "CARGO_HOME=/tmp/cargo-home cargo check",
            "CARGO_HOME raw cache override must be classified",
        ),
        (
            "RUSTUP_HOME=/tmp/rustup-home cargo check",
            "RUSTUP_HOME raw toolchain override must be classified",
        ),
        (
            "RUSTFLAGS='--out-dir /tmp/raw-out' cargo check",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "RUSTFLAGS='--artifact-dir /tmp/raw-artifacts' cargo check",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  RUSTFLAGS: '--out-dir /tmp/raw-out'",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  \"RUSTFLAGS\": '--out-dir /tmp/raw-out'",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "RUSTC_WRAPPER=/tmp/wrapper cargo check",
            "RUSTC_WRAPPER raw compiler wrapper must be classified",
        ),
        (
            "RUSTC_WORKSPACE_WRAPPER=/tmp/workspace-wrapper cargo check",
            "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified",
        ),
        (
            "cargo rustc -- --out-dir /tmp/raw-out",
            "cargo rustc --out-dir raw output override must be classified",
        ),
        (
            "/tmp/myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "cargo rustc -- --artifact-dir /tmp/raw-artifacts",
            "cargo rustc --artifact-dir raw output override must be classified",
        ),
        (
            "cargo install ripgrep --target x86_64-unknown-linux-gnu --target-dir /tmp/install-build --root /tmp/install-root",
            "cargo install build target and install root ownership must be classified separately",
        ),
        (
            "cargo install ripgrep --root s3://bolt-v2-active-cache/install-root",
            "cargo install S3 install root must be classified",
        ),
        (
            "/tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "timeout 30 /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/builder build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "exec -a name cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "exec -cla name cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --rm -v $PWD:/workspace -w /workspace rust:latest cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --label my-label rust cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --unknown-opt=rust mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "podman run --unknown-opt=rust myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "env >output.log /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "env 1=2 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "timeout -- 30 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "taskset -- 0 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "runuser -u user /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "sudo sudo sudo sudo sudo sudo sudo /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/c install cargo-deny --root s3://bolt-v2-active-cache/install-root",
            "cargo install S3 install root must be classified",
        ),
        (
            "/tmp/c --config build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "alias c=cargo; c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "alias c=cargo\nc build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "alias mybuild=cargo; alias c=mybuild; c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo --config=build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config /tmp/cargo-config.toml check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo --config myconfig.txt check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo --config=myconfig.txt check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo install ripgrep --root /tmp/install-root --target-dir /tmp/install-build",
            "cargo install build target and install root ownership must be classified separately",
        ),
        (
            "BOLT_MANAGED_JUST=1 just managed-build",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "VAR=BOLT_MANAGED_JUST; export $VAR=1; just managed-build",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "echo \"BOLT_MANAGED_JUST<<EOF\" >> \"$GITHUB_ENV\"",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "no-mistakes run -- cargo check",
            "no-mistakes raw Cargo drift must be classified",
        ),
        (
            "run: >\n  no-mistakes run --\n  cargo check",
            "no-mistakes raw Cargo drift must be classified",
        ),
        (
            "no-mistakes run --worktree . -- cargo check --target-dir target",
            "no-mistakes worktree-local target path evidence must be reported",
        ),
        (
            "aws s3 sync target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync $(echo target) s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'bash -c "aws s3 sync target s3://bolt-v2-active-cache/target"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp ./$(echo target)/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp ./`echo target`/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp `echo target`/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp $(echo file) target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp `echo`target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(echo target)" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'export E=CARGO_TARGET_DIR; env FOO=bar bash -c "$E=/tmp/raw cargo check"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; declare -x $E=/tmp/raw; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "cargo check $(echo ; echo --target-dir /tmp/raw)",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'SRC_DIR=target/debug\naws s3 sync "$SRC_DIR" s3://bolt-v2-active-cache/target/debug',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'env:\n  DEST: s3://bucket/cache\nsteps:\n  - run: aws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'env:\n  DEST: s3://bucket/cache\nsteps:\n  - run: aws s3 sync target "${{ env.DEST }}"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'echo "DEST=s3://bucket/cache" >> "$GITHUB_ENV"\naws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'DEST="s3://bolt-v2-active-cache/target"\naws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'SRC="s3://bolt-v2-active-cache/target"\naws s3 sync "$SRC" target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "run: |\n  aws s3 sync \\\n    target \\\n    s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "run: >\n  aws s3 sync\n  target\n  s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync ./target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(pwd)/target" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$CARGO_TARGET_DIR\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR}\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR%/}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir }}\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir_relative }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(python3 scripts/rust_verification.py target-dir --repo .)" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 mv --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws --profile prod s3 sync target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws --region us-east-1 s3 cp --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE\" s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync . s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE\"/target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'TARGET_DIR="$GITHUB_WORKSPACE"/target\nDEST=s3://bucket/cache\naws s3 sync "$TARGET_DIR" "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'TARGET_DIR="$PWD"/target\nDEST=s3://bucket/cache\naws s3 sync "$TARGET_DIR" "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"$GITHUB_WORKSPACE/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp --recursive s3://bolt-v2-active-cache/target \"${PWD}/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 mv --recursive s3://bolt-v2-active-cache/target \"$PWD/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ github.workspace }}/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ github.workspace }}\" s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ env.CARGO_TARGET_DIR }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$PWD/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$PWD\"/target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync $PWD/ s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync /home/runner/work/bolt-v2/bolt-v2/target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target && aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target/debug && aws s3 sync * s3://bolt-v2-active-cache/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "pushd target/debug\naws s3 sync * s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target ; aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target || aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"$CARGO_TARGET_DIR\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"${{ steps.setup.outputs.managed_target_dir }}\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api put-object --bucket bolt-v2-active-cache --key target/debug/lib.rmeta --body target/debug/lib.rmeta",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api put-object --bucket bolt-v2-active-cache --key target/debug/lib.rmeta --body \"$CARGO_TARGET_DIR/debug/lib.rmeta\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api get-object --bucket bolt-v2-active-cache --key cache target/debug/lib.rmeta",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cat target/debug/lib.rmeta | base64 | aws s3 cp - s3://bolt-v2-active-cache/target.rmeta.b64",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "head -c 999 target/debug/lib.rmeta | aws s3 cp - s3://bolt-v2-active-cache/file",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "tar c target |\naws s3 cp - s3://bolt-v2-active-cache/target.tar",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp $(echo ;) target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
    ]
    verifier = load_verifier()
    misses: list[str] = []
    for script, fragment in cases:
        errors = verifier.verify_text(workflow_with_detector_probe(script), BASE_ACTION, BASE_NEXTEST_CONFIG)
        if not any(fragment in error for error in errors):
            misses.append(f"{fragment!r}: errors={errors!r}")
    false_positive = "alias c=cargo; echo c build --target-dir /tmp/raw-target"
    errors = verifier.raw_rust_storage_errors(false_positive)
    if "cargo --target-dir raw target override must be classified" in errors:
        misses.append(f"non-executed alias text was classified: errors={errors!r}")
    false_positive = "aws s3 cp dist/app s3://bolt-v2-release/app\necho target"
    errors = verifier.raw_rust_storage_errors(false_positive)
    if "S3 active mutable target cache must be rejected" in errors:
        misses.append(f"separate-line non-target S3 upload was classified: errors={errors!r}")
    for false_positive in (
        "cargo test -- --target-dir /tmp/test-binary-arg",
        "cargo nextest run -- --target-dir /tmp/test-binary-arg",
        "python3 scripts/rust_verification.py cargo --repo . -- test -- --target-dir /tmp/test-binary-arg",
        "python3 scripts/rust_verification.py run --repo . test -- --target-dir /tmp/test-binary-arg",
    ):
        errors = verifier.raw_rust_storage_errors(false_positive)
        if (
            "cargo --target-dir raw target override must be classified" in errors
            or "S3 active mutable target cache must be rejected" in errors
        ):
            misses.append(f"benign command was classified: {false_positive!r} errors={errors!r}")
    for false_positive in ("/usr/bin/make build", "/tmp/build-tool test", "cargo -C /tmp/repo build"):
        errors = verifier.raw_rust_storage_errors(false_positive)
        if any("raw target override" in error or "raw Cargo drift" in error for error in errors):
            misses.append(f"path command was classified as raw cargo: {false_positive!r} errors={errors!r}")
    if misses:
        raise AssertionError("raw/unmanaged Rust storage policy gaps were silent: " + "; ".join(misses))

def assert_v6_red_yaml_anchor_jobs_do_not_hide_raw_storage() -> None:
    verifier = load_verifier()
    workflow = """
name: Probe
on: [push]
jobs:
  hidden: &hidden
    runs-on: ubuntu-latest
    steps:
      - run: cargo build --target-dir /tmp/raw
"""
    expected = "cargo --target-dir raw target override must be classified"
    errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow))
    if expected not in errors:
        raise AssertionError(f"anchored workflow job raw-storage drift was silent: {errors!r}")
    if "hidden" not in verifier.parse_jobs(textwrap.dedent(workflow)):
        raise AssertionError("anchored workflow job was not parsed")

def assert_v6_red_yaml_anchor_steps_do_not_hide_raw_storage() -> None:
    verifier = load_verifier()
    workflow = """
name: Probe
on: [push]
jobs:
  hidden:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
      - &raw run: cargo build --target-dir /tmp/raw
      - &s3 run: aws s3 sync target s3://bolt-v2-active-cache/target
"""
    errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow))
    expected_target = "cargo --target-dir raw target override must be classified"
    expected_s3 = "S3 active mutable target cache must be rejected"
    if expected_target not in errors or expected_s3 not in errors:
        raise AssertionError(f"anchored workflow step raw-storage drift was silent: {errors!r}")

def assert_v6_red_raw_storage_checks_all_ci_automation() -> None:
    verifier = load_verifier()
    advisory = BASE_ADVISORY_WORKFLOW.replace(
        "        run: just deny-advisories",
        "        run: |\n          aws s3 sync target s3://some-bucket/linux-cache",
    )
    advisory_errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    action = BASE_ACTION.replace(
        "      run: echo setup",
        "      run: |\n        CARGO_TARGET_DIR=/tmp/raw cargo check",
    )
    action_errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW},
        action,
        BASE_NEXTEST_CONFIG,
    )
    repo_errors = verifier.verify_repo_automation_texts(
        {
            "justfile": "check:\n    CARGO_TARGET_DIR=/tmp/raw cargo check\n",
            "justfile.raw": "test:\n    cargo test\n",
            "justfile.spoof": 'bad:\n    echo "BOLT_MANAGED_JUST exit"\n    cargo build\n',
            "justfile.managed-spoof": 'managed-build:\n    echo BOLT_MANAGED_JUST rust_verification.py run exit 2\n    cargo build\n',
            "justfile.managed-exact-guard-raw": 'managed-build:\n    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-build must run through scripts/rust_verification.py run"; exit 2; fi\n    cargo build --release\n',
            "scripts/raw.sh": "#!/usr/bin/env bash\ncargo build\n",
            "scripts/raw-substitution-dollar.sh": "#!/usr/bin/env bash\nx=$(cargo build)\n",
            "scripts/raw-substitution-quoted.sh": "#!/usr/bin/env bash\nx=\"$(cargo build)\"\n",
            "scripts/raw-substitution-backtick.sh": "#!/usr/bin/env bash\nx=`cargo build`\n",
            "scripts/raw-find-exec.sh": "#!/usr/bin/env bash\nfind . -name Cargo.toml -exec cargo build \\;\n",
            "scripts/raw-su.sh": "#!/usr/bin/env bash\nsu user -c 'cargo build'\n",
            "scripts/raw-su-shell-arg.sh": "#!/usr/bin/env bash\nsu -s -c user -c 'cargo build'\n",
            "scripts/raw-runuser.sh": "#!/usr/bin/env bash\nrunuser -u user -- cargo build\n",
            "scripts/raw-flock.sh": "#!/usr/bin/env bash\nflock /tmp/lock -ccargo\\ build\n",
            "scripts/raw-flock-option-arg.sh": "#!/usr/bin/env bash\nflock -w -c /tmp/lock -c 'cargo build'\n",
            "scripts/raw-chrt-batch.sh": "#!/usr/bin/env bash\nchrt -b cargo build\n",
            "scripts/raw-env-argv0.sh": "#!/usr/bin/env bash\nenv --argv0 cargo cargo build\n",
            "scripts/multiline-eval.sh": "#!/usr/bin/env bash\nCMD=\"cargo build\"\nbash -c \"$CMD\"\n",
            "scripts/multiline-quoted-eval.sh": "#!/usr/bin/env bash\nCMD=\"cargo\nbuild --target-dir /tmp/raw\"\nbash -c \"$CMD\"\n",
            "scripts/comment-blind.sh": "# comment with unbalanced quote '\ncargo build\necho 'closing quote'\n",
            "scripts/nested-var-eval.sh": "CMD=\"cargo build\"\nbash -c \"echo benign; eval $CMD\"\n",
            "scripts/raw-guard-text.sh": '#!/usr/bin/env bash\necho "Missing BOLT_MANAGED_JUST, exit 1"\ncargo build\n',
            "scripts/raw-redirection.sh": "#!/usr/bin/env bash\n> /dev/null cargo build\n",
            "scripts/symlink-cargo.sh": "ln -s $(which cargo) /tmp/mycargo\n/tmp/mycargo build --target-dir /tmp/raw\n",
            "scripts/copy-cargo.sh": "cp $(which cargo) /tmp/mycargo\n/tmp/mycargo build\n",
            "scripts/non-rust-make.sh": "/usr/bin/make test\n",
            "scripts/non-rust-gradle.sh": "./gradlew build\n",
            "scripts/non-rust-cargo-build-script.sh": "/tmp/cargo-build.sh test\n",
            "scripts/non-rust-cargo-build-uppercase-py.sh": "/tmp/cargo-build.PY test\n",
            "scripts/non-rust-cargo-tests-py.sh": "tests/cargo-tests.py build\n",
            "justfile.setup": "setup:\n    cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.absolute": "setup:\n    /usr/bin/cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.timeout": "setup:\n    timeout 30 cargo install cargo-deny --version 0.18.2\n",
            "justfile.setup.xargs": "setup:\n    xargs cargo install cargo-nextest\n",
            "scripts/local.sh": "aws s3 sync \"$PWD\"/target s3://some-bucket/linux-cache\n",
            "scripts/workspace.sh": "aws s3 sync \"$GITHUB_WORKSPACE\" s3://some-bucket/workspace\n",
            "scripts/nested-s3-shell.sh": 'bash -c "aws s3 sync target s3://bolt-v2-active-cache/target"\n',
            "scripts/export-name-word.sh": "export E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check\n",
            "scripts/s3api.sh": "aws s3api put-object --bucket b --key target/debug/lib --body target/debug/lib\n",
            "scripts/s3api-get.sh": "aws s3api get-object --bucket b --key cache target/debug/lib\n",
        }
    )
    expected = "S3 active mutable target cache must be rejected"
    if not any(expected in error for error in advisory_errors):
        raise AssertionError(f"advisory workflow raw-storage drift was silent: {advisory_errors!r}")
    expected = "CARGO_TARGET_DIR raw target override must be classified"
    if not any(expected in error for error in action_errors):
        raise AssertionError(f"setup action raw-storage drift was silent: {action_errors!r}")
    if not any("justfile" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile raw-storage drift was silent: {repo_errors!r}")
    expected = "repo automation raw Cargo must use managed rust_verification wrapper"
    if not any("justfile.raw" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile raw-cargo drift was silent: {repo_errors!r}")
    if not any("justfile.spoof" in error and expected in error for error in repo_errors):
        raise AssertionError(f"spoofed justfile managed-guard drift was silent: {repo_errors!r}")
    if not any("justfile.managed-spoof" in error and expected in error for error in repo_errors):
        raise AssertionError(f"spoofed managed just recipe guard drift was silent: {repo_errors!r}")
    if not any("justfile.managed-exact-guard-raw" in error and expected in error for error in repo_errors):
        raise AssertionError(f"exact-guard managed just recipe raw cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-dollar.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script command-substitution raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-quoted.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script quoted command-substitution raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-backtick.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script backtick raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-find-exec.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script find-exec raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-su.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script su raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-su-shell-arg.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script su shell-arg raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-runuser.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script runuser raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-flock.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script flock raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-flock-option-arg.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script flock option-arg raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/multiline-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script multiline eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/multiline-quoted-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script multiline quoted eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/comment-blind.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script comment-blinded raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/nested-var-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script nested variable eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-guard-text.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script guard-text raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-redirection.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script redirected raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/symlink-cargo.sh" in error and "cargo --target-dir raw target override" in error for error in repo_errors):
        raise AssertionError(f"symlinked cargo raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/copy-cargo.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"copied cargo raw-cargo drift was silent: {repo_errors!r}")
    false_repo_raw = [error for error in repo_errors if "scripts/non-rust-" in error]
    if false_repo_raw:
        raise AssertionError(f"non-Rust path commands must stay allowed: {false_repo_raw!r}")
    expected = "repo automation must not compile cargo-nextest from source"
    if not any("justfile.setup" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile cargo-install drift was silent: {repo_errors!r}")
    if not any("justfile.setup.absolute" in error and expected in error for error in repo_errors):
        raise AssertionError(f"absolute cargo-install drift was silent: {repo_errors!r}")
    expected = "repo automation must not compile cargo-deny from source"
    if not any("justfile.setup.timeout" in error and expected in error for error in repo_errors):
        raise AssertionError(f"wrapped cargo-deny install drift was silent: {repo_errors!r}")
    expected = "repo automation must not compile cargo-nextest from source"
    if not any("justfile.setup.xargs" in error and expected in error for error in repo_errors):
        raise AssertionError(f"wrapped cargo-nextest install drift was silent: {repo_errors!r}")
    expected = "S3 active mutable target cache must be rejected"
    if not any("scripts/local.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/workspace.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"workspace S3 sync drift was silent: {repo_errors!r}")
    if not any("scripts/nested-s3-shell.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"nested shell S3 sync drift was silent: {repo_errors!r}")
    expected = "CARGO_TARGET_DIR raw target override must be classified"
    if not any("scripts/export-name-word.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"export name-word raw-storage drift was silent: {repo_errors!r}")
    expected = "S3 active mutable target cache must be rejected"
    if not any("scripts/s3api.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/s3api-get.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api get-object raw-storage drift was silent: {repo_errors!r}")


def main() -> int:
    assert_v6_deploy_artifact_s3_stays_allowed()
    assert_v6_red_s3_storage_transfer_policy_is_semantic()
    assert_v6_workflow_run_steps_reset_shell_state()
    assert_v6_red_raw_rust_storage_overrides_are_reported()
    assert_v6_red_yaml_anchor_jobs_do_not_hide_raw_storage()
    assert_v6_red_yaml_anchor_steps_do_not_hide_raw_storage()
    assert_v6_red_raw_storage_checks_all_ci_automation()
    print("OK: shell dataflow analysis tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
