#!/usr/bin/env python3
"""Behavior tests for governed strict-sccache build metadata."""

from __future__ import annotations

import copy
import contextlib
import dataclasses
import hashlib
import io
import json
import pathlib
import sys
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import sccache_strict  # noqa: E402


SOURCE_COMMIT = "b799af2eea02bba9e0ef2550775fe10296b62981"
SOURCE_SHA256 = "a4419b0a2278255d11eda1f76ee98efab0aec72649617bbefd24a5e92acf4af3"
CONTAINER_DIGEST = (
    "sha256:76df925f30e106755517c78cd57b6ea890a73d6f59fcff842849006e734c174e"
)
RUSTC_COMMIT = "8bab26f4f68e0e26f0bb7960be334d5b520ea452"


def valid_document() -> dict[str, object]:
    return {
        "schema_version": 1,
        "source": {
            "version": "0.16.0",
            "commit": SOURCE_COMMIT,
            "archive_url": f"https://github.com/mozilla/sccache/archive/{SOURCE_COMMIT}.tar.gz",
            "archive_sha256": SOURCE_SHA256,
            "source_date_epoch": 1_781_869_188,
            "patch": "ci/sccache-strict/sccache-v0.16.0-strict.patch",
        },
        "build": {
            "container": f"docker.io/clux/muslrust@{CONTAINER_DIGEST}",
            "rustc_release": "1.97.1",
            "rustc_commit": RUSTC_COMMIT,
            "features": ["s3", "vendored-openssl"],
            "default_features": False,
            "profile": "release",
            "workflow": ".github/workflows/sccache-strict-release.yml",
            "recipe": "scripts/build_strict_sccache.sh",
            "driver": "scripts/sccache_strict.py",
        },
        "verification": {
            "strict_timeout_ms": 1_000,
            "max_frame_bytes": 16_777_216,
            "cache_mode": "READ_WRITE",
            "replicas": ["a", "b"],
            "attestation_attempts": 12,
            "attestation_interval_seconds": 10,
        },
        "targets": {
            "ARM64": {
                "triple": "aarch64-unknown-linux-musl",
                "elf_machine": "AArch64",
            },
            "X64": {
                "triple": "x86_64-unknown-linux-musl",
                "elf_machine": "Advanced Micro Devices X86-64",
            },
        },
    }


class LoadConfigTests(unittest.TestCase):
    def test_loads_governed_verification_timeout(self) -> None:
        config = sccache_strict.load_document(valid_document(), repo_root=REPO_ROOT)

        self.assertEqual(config.verification_timeout_ms, 1_000)
        self.assertEqual(config.max_frame_bytes, 16_777_216)

    def test_rejects_non_positive_verification_timeout(self) -> None:
        document = valid_document()
        verification = copy.deepcopy(document["verification"])
        assert isinstance(verification, dict)
        verification["strict_timeout_ms"] = 0
        document["verification"] = verification

        with self.assertRaisesRegex(
            ValueError, "verification.strict_timeout_ms must be a positive integer"
        ):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_frame_limit_larger_than_protocol_field(self) -> None:
        document = valid_document()
        verification = copy.deepcopy(document["verification"])
        assert isinstance(verification, dict)
        verification["max_frame_bytes"] = 2**32
        document["verification"] = verification

        with self.assertRaisesRegex(ValueError, "positive 32-bit integer"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_unknown_verification_cache_mode(self) -> None:
        document = valid_document()
        verification = copy.deepcopy(document["verification"])
        assert isinstance(verification, dict)
        verification["cache_mode"] = "read-write"
        document["verification"] = verification

        with self.assertRaisesRegex(
            ValueError, "verification.cache_mode must be READ_ONLY or READ_WRITE"
        ):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_loads_exact_pinned_build(self) -> None:
        config = sccache_strict.load_config(REPO_ROOT / "ci/sccache-strict.toml")

        self.assertEqual(config.schema_version, 1)
        self.assertEqual(config.source_commit, SOURCE_COMMIT)
        self.assertEqual(config.source_sha256, SOURCE_SHA256)
        self.assertEqual(config.container_digest, CONTAINER_DIGEST)
        self.assertEqual(config.rustc_commit, RUSTC_COMMIT)
        self.assertEqual(config.features, ("s3", "vendored-openssl"))
        self.assertFalse(config.default_features)
        self.assertEqual(config.verification_cache_mode, "READ_WRITE")
        self.assertEqual(config.replicas, ("a", "b"))
        self.assertEqual(config.attestation_attempts, 12)
        self.assertEqual(config.attestation_interval_seconds, 10)
        self.assertEqual(set(config.targets), {"ARM64", "X64"})

    def test_rejects_unknown_top_level_key(self) -> None:
        document = valid_document()
        document["unexpected"] = True

        with self.assertRaisesRegex(ValueError, "unknown top-level key"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_non_integer_schema_version(self) -> None:
        document = valid_document()
        document["schema_version"] = 1.0

        with self.assertRaisesRegex(ValueError, "schema_version must be integer 1"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_unknown_build_key(self) -> None:
        document = valid_document()
        build = copy.deepcopy(document["build"])
        assert isinstance(build, dict)
        build["extra"] = "forbidden"
        document["build"] = build

        with self.assertRaisesRegex(ValueError, "unknown build key"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_mutable_container_reference(self) -> None:
        document = valid_document()
        build = copy.deepcopy(document["build"])
        assert isinstance(build, dict)
        build["container"] = "docker.io/clux/muslrust:stable"
        document["build"] = build

        with self.assertRaisesRegex(ValueError, "container must use sha256 digest"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_boolean_source_epoch(self) -> None:
        document = valid_document()
        source = copy.deepcopy(document["source"])
        assert isinstance(source, dict)
        source["source_date_epoch"] = True
        document["source"] = source

        with self.assertRaisesRegex(
            ValueError, "source_date_epoch must be a positive integer"
        ):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_target_outside_governed_architectures(self) -> None:
        document = valid_document()
        targets = copy.deepcopy(document["targets"])
        assert isinstance(targets, dict)
        targets["RISCV64"] = {
            "triple": "riscv64gc-unknown-linux-musl",
            "elf_machine": "RISC-V",
        }
        document["targets"] = targets

        with self.assertRaisesRegex(
            ValueError, "targets must be exactly ARM64 and X64"
        ):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_rejects_duplicate_replica_names(self) -> None:
        document = valid_document()
        verification = copy.deepcopy(document["verification"])
        assert isinstance(verification, dict)
        verification["replicas"] = ["same", "same"]
        document["verification"] = verification

        with self.assertRaisesRegex(ValueError, "two unique governed names"):
            sccache_strict.load_document(document, repo_root=REPO_ROOT)

    def test_accepts_additional_governed_replica(self) -> None:
        document = valid_document()
        verification = copy.deepcopy(document["verification"])
        assert isinstance(verification, dict)
        verification["replicas"] = ["a", "b", "c"]
        document["verification"] = verification

        config = sccache_strict.load_document(document, repo_root=REPO_ROOT)

        self.assertEqual(config.replicas, ("a", "b", "c"))


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        self.repo_root = self.root / "repo"
        self.repo_root.mkdir()
        self.patch_path = (
            self.repo_root / "ci/sccache-strict/sccache-v0.16.0-strict.patch"
        )
        self.patch_path.parent.mkdir(parents=True)
        self.patch_path.write_bytes(b"strict patch bytes\n")
        self.workflow_path = (
            self.repo_root / ".github/workflows/sccache-strict-release.yml"
        )
        self.workflow_path.parent.mkdir(parents=True)
        self.workflow_path.write_bytes(b"strict workflow bytes\n")
        self.recipe_path = self.repo_root / "scripts/build_strict_sccache.sh"
        self.recipe_path.parent.mkdir(parents=True)
        self.recipe_path.write_bytes(b"strict recipe bytes\n")
        self.driver_path = self.repo_root / "scripts/sccache_strict.py"
        self.driver_path.write_bytes(b"strict driver bytes\n")
        self.config = sccache_strict.load_document(
            valid_document(), repo_root=self.repo_root
        )
        self.repository = "seungpyoson/bolt-v2"
        self.run_id = "29814080045"
        self.run_attempt = "1"
        self.head_sha = "1" * 40
        self.binary_paths: dict[tuple[str, str], pathlib.Path] = {}
        self.manifests: list[dict[str, object]] = []
        for architecture, binary in (
            ("ARM64", b"arm64-binary"),
            ("X64", b"x64-binary"),
        ):
            for replica in ("a", "b"):
                binary_path = self.root / f"{architecture}-{replica}"
                binary_path.write_bytes(binary)
                manifest_path = self.root / f"{architecture}-{replica}.json"
                manifest = sccache_strict.write_candidate_manifest(
                    output_path=manifest_path,
                    config=self.config,
                    binary_path=binary_path,
                    repository=self.repository,
                    run_id=self.run_id,
                    run_attempt=self.run_attempt,
                    head_sha=self.head_sha,
                    architecture=architecture,
                    replica=replica,
                )
                self.assertEqual(json.loads(manifest_path.read_text()), manifest)
                self.binary_paths[(architecture, replica)] = binary_path
                self.manifests.append(manifest)

    def verify(self) -> sccache_strict.VerifiedCandidateSet:
        return sccache_strict.verify_candidate_set(
            self.manifests,
            self.binary_paths,
            config=self.config,
            repository=self.repository,
            run_id=self.run_id,
            run_attempt=self.run_attempt,
            head_sha=self.head_sha,
        )

    def test_verifies_exact_four_candidate_set(self) -> None:
        verified = self.verify()

        self.assertEqual(set(verified.assets), {"ARM64", "X64"})
        self.assertTrue(
            verified.release_tag.startswith("tooling-sccache-v0.16.0-strict-")
        )
        self.assertEqual(len(verified.provenance_sha256), 64)

    def test_release_tag_changes_when_governed_build_inputs_change(self) -> None:
        original = self.verify()
        document = valid_document()
        build = copy.deepcopy(document["build"])
        assert isinstance(build, dict)
        build["container"] = f"docker.io/clux/muslrust@sha256:{'1' * 64}"
        document["build"] = build
        changed_config = sccache_strict.load_document(
            document, repo_root=self.repo_root
        )
        changed_manifests: list[dict[str, object]] = []
        for architecture in ("ARM64", "X64"):
            for replica in ("a", "b"):
                changed_manifests.append(
                    sccache_strict.write_candidate_manifest(
                        output_path=self.root
                        / f"changed-{architecture}-{replica}.json",
                        config=changed_config,
                        binary_path=self.binary_paths[(architecture, replica)],
                        repository=self.repository,
                        run_id=self.run_id,
                        run_attempt=self.run_attempt,
                        head_sha=self.head_sha,
                        architecture=architecture,
                        replica=replica,
                    )
                )

        changed = sccache_strict.verify_candidate_set(
            changed_manifests,
            self.binary_paths,
            config=changed_config,
            repository=self.repository,
            run_id=self.run_id,
            run_attempt=self.run_attempt,
            head_sha=self.head_sha,
        )

        self.assertNotEqual(original.release_tag, changed.release_tag)

    def test_release_tag_binds_workflow_and_recipe_bytes(self) -> None:
        original = self.verify()
        original_workflow = self.workflow_path.read_bytes()
        original_recipe = self.recipe_path.read_bytes()

        for index, governed_path in enumerate(
            (self.workflow_path, self.recipe_path, self.driver_path), start=1
        ):
            self.workflow_path.write_bytes(original_workflow)
            self.recipe_path.write_bytes(original_recipe)
            self.driver_path.write_bytes(b"strict driver bytes\n")
            governed_path.write_bytes(f"changed-{index}\n".encode())
            manifests: list[dict[str, object]] = []
            for architecture in ("ARM64", "X64"):
                for replica in self.config.replicas:
                    manifests.append(
                        sccache_strict.write_candidate_manifest(
                            output_path=self.root
                            / f"identity-{index}-{architecture}-{replica}.json",
                            config=self.config,
                            binary_path=self.binary_paths[(architecture, replica)],
                            repository=self.repository,
                            run_id=self.run_id,
                            run_attempt=self.run_attempt,
                            head_sha=self.head_sha,
                            architecture=architecture,
                            replica=replica,
                        )
                    )
            changed = sccache_strict.verify_candidate_set(
                manifests,
                self.binary_paths,
                config=self.config,
                repository=self.repository,
                run_id=self.run_id,
                run_attempt=self.run_attempt,
                head_sha=self.head_sha,
            )
            self.assertNotEqual(original.release_tag, changed.release_tag)
        self.workflow_path.write_bytes(original_workflow)
        self.recipe_path.write_bytes(original_recipe)

    def test_provenance_build_identity_recomputes_release_identity(self) -> None:
        verified = self.verify()
        provenance = json.loads(verified.provenance_bytes)
        identity_bytes = (
            json.dumps(
                provenance["build_identity"],
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n"
        ).encode("utf-8")

        self.assertEqual(
            hashlib.sha256(identity_bytes).hexdigest(),
            provenance["build_identity_sha256"],
        )
        self.assertTrue(
            verified.release_tag.endswith(provenance["build_identity_sha256"])
        )

    def test_attestation_retry_policy_changes_release_identity(self) -> None:
        original = self.verify()
        changed_config = dataclasses.replace(
            self.config,
            attestation_attempts=self.config.attestation_attempts + 1,
        )

        changed = sccache_strict.verify_candidate_set(
            self.manifests,
            self.binary_paths,
            config=changed_config,
            repository=self.repository,
            run_id=self.run_id,
            run_attempt=self.run_attempt,
            head_sha=self.head_sha,
        )

        self.assertNotEqual(original.release_tag, changed.release_tag)

    def test_rejects_cross_run_manifest(self) -> None:
        self.manifests[0] = copy.deepcopy(self.manifests[0])
        self.manifests[0]["run_id"] = "other-run"

        with self.assertRaisesRegex(ValueError, "same workflow run"):
            self.verify()

    def test_rejects_cross_attempt_manifest(self) -> None:
        self.manifests[0] = copy.deepcopy(self.manifests[0])
        self.manifests[0]["run_attempt"] = "2"

        with self.assertRaisesRegex(ValueError, "same workflow attempt"):
            self.verify()

    def test_rejects_changed_binary_bytes(self) -> None:
        self.binary_paths[("ARM64", "b")].write_bytes(b"changed")

        with self.assertRaisesRegex(ValueError, "binary digest"):
            self.verify()

    def test_manifest_output_must_not_preexist(self) -> None:
        output = self.root / "existing.json"
        output.write_bytes(b"keep")

        with self.assertRaisesRegex(ValueError, "unable to create output"):
            sccache_strict.write_candidate_manifest(
                output_path=output,
                config=self.config,
                binary_path=self.binary_paths[("ARM64", "a")],
                repository=self.repository,
                run_id=self.run_id,
                run_attempt=self.run_attempt,
                head_sha=self.head_sha,
                architecture="ARM64",
                replica="a",
            )

        self.assertEqual(output.read_bytes(), b"keep")

    def test_manifest_output_must_be_outside_repository(self) -> None:
        with self.assertRaisesRegex(ValueError, "outside the repository"):
            sccache_strict.write_candidate_manifest(
                output_path=self.repo_root / "candidate.json",
                config=self.config,
                binary_path=self.binary_paths[("ARM64", "a")],
                repository=self.repository,
                run_id=self.run_id,
                run_attempt=self.run_attempt,
                head_sha=self.head_sha,
                architecture="ARM64",
                replica="a",
            )

    def test_rejects_boolean_candidate_schema(self) -> None:
        self.manifests[0] = copy.deepcopy(self.manifests[0])
        self.manifests[0]["schema_version"] = True

        with self.assertRaisesRegex(
            ValueError, "candidate schema_version must be integer 1"
        ):
            self.verify()


class PublishGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.sha = "2" * 40
        self.environment = {
            "name": "strict-sccache-publisher",
            "deployment_branch_policy": {
                "protected_branches": True,
                "custom_branch_policies": False,
            },
        }

    def validate(self) -> None:
        sccache_strict.validate_publish_context(
            event_name="workflow_dispatch",
            requested_sha=self.sha,
            event_sha=self.sha,
            remote_main_sha=self.sha,
            event_ref="refs/heads/main",
            environment_document=self.environment,
            environment_name="strict-sccache-publisher",
        )

    def test_accepts_exact_main_with_protected_environment(self) -> None:
        self.validate()

    def test_rejects_missing_environment(self) -> None:
        self.environment = {}

        with self.assertRaisesRegex(ValueError, "publisher environment is absent"):
            self.validate()


class ReleaseRecordTests(unittest.TestCase):
    def test_requires_immutable_release_and_exact_assets(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            repo_root = root / "repo"
            patch_path = repo_root / "ci/sccache-strict/sccache-v0.16.0-strict.patch"
            patch_path.parent.mkdir(parents=True)
            patch_path.write_bytes(b"patch")
            workflow_path = repo_root / ".github/workflows/sccache-strict-release.yml"
            workflow_path.parent.mkdir(parents=True)
            workflow_path.write_bytes(b"workflow")
            recipe_path = repo_root / "scripts/build_strict_sccache.sh"
            recipe_path.parent.mkdir(parents=True)
            recipe_path.write_bytes(b"recipe")
            driver_path = repo_root / "scripts/sccache_strict.py"
            driver_path.write_bytes(b"driver")
            config = sccache_strict.load_document(valid_document(), repo_root=repo_root)
            manifests: list[dict[str, object]] = []
            binaries: dict[tuple[str, str], pathlib.Path] = {}
            for architecture in ("ARM64", "X64"):
                for replica in ("a", "b"):
                    binary_path = root / f"{architecture}-{replica}"
                    binary_path.write_bytes(architecture.encode("ascii"))
                    binaries[(architecture, replica)] = binary_path
                    manifests.append(
                        sccache_strict.write_candidate_manifest(
                            output_path=root / f"{architecture}-{replica}.json",
                            config=config,
                            binary_path=binary_path,
                            repository="seungpyoson/bolt-v2",
                            run_id="123",
                            run_attempt="1",
                            head_sha="3" * 40,
                            architecture=architecture,
                            replica=replica,
                        )
                    )
            verified = sccache_strict.verify_candidate_set(
                manifests,
                binaries,
                config=config,
                repository="seungpyoson/bolt-v2",
                run_id="123",
                run_attempt="1",
                head_sha="3" * 40,
            )
            release = {
                "tag_name": verified.release_tag,
                "target_commitish": verified.head_sha,
                "draft": False,
                "immutable": False,
                "assets": [
                    {
                        "name": asset.name,
                        "size": asset.size,
                        "digest": f"sha256:{asset.sha256}",
                    }
                    for asset in verified.assets.values()
                ]
                + [
                    {
                        "name": verified.provenance_name,
                        "size": len(verified.provenance_bytes),
                        "digest": f"sha256:{verified.provenance_sha256}",
                    }
                ],
            }
            tag_ref = {
                "ref": f"refs/tags/{verified.release_tag}",
                "object": {"type": "commit", "sha": verified.head_sha},
            }

            with self.assertRaisesRegex(ValueError, "release is not immutable"):
                sccache_strict.verify_release_record(verified, release, tag_ref)

            release["immutable"] = True
            sccache_strict.verify_release_record(verified, release, tag_ref)

            tag_ref["object"] = {"type": "commit", "sha": "4" * 40}
            with self.assertRaisesRegex(ValueError, "exact head commit"):
                sccache_strict.verify_release_record(verified, release, tag_ref)


class ReleaseCleanupTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repository = "owner/repository"
        self.run_id = "123"
        self.run_attempt = "2"
        self.head_sha = "a" * 40
        self.tag = "tooling-sccache-v0.16.0-strict-" + "b" * 64
        self.marker = sccache_strict.release_ownership_marker(
            repository=self.repository,
            run_id=self.run_id,
            run_attempt=self.run_attempt,
            head_sha=self.head_sha,
            tag=self.tag,
        )

    def release(
        self, *, release_id: int = 77, body: str | None = None
    ) -> dict[str, object]:
        return {
            "id": release_id,
            "tag_name": self.tag,
            "target_commitish": self.head_sha,
            "draft": True,
            "immutable": False,
            "body": self.marker if body is None else body,
        }

    def test_recovers_exact_owned_draft_when_create_response_is_lost(self) -> None:
        release_id = sccache_strict.select_owned_mutable_release(
            [[self.release()]],
            expected_id=None,
            tag=self.tag,
            head_sha=self.head_sha,
            ownership_marker=self.marker,
        )

        self.assertEqual(release_id, 77)

    def test_selects_exact_owned_mutable_published_release(self) -> None:
        published = self.release()
        published["draft"] = False

        release_id = sccache_strict.select_owned_mutable_release(
            published,
            expected_id=77,
            tag=self.tag,
            head_sha=self.head_sha,
            ownership_marker=self.marker,
        )

        self.assertEqual(release_id, 77)

    def test_refuses_foreign_or_ambiguous_draft_recovery(self) -> None:
        foreign = self.release(body="different publisher")
        duplicate = self.release(release_id=78)

        self.assertIsNone(
            sccache_strict.select_owned_mutable_release(
                [[foreign]],
                expected_id=None,
                tag=self.tag,
                head_sha=self.head_sha,
                ownership_marker=self.marker,
            )
        )
        self.assertIsNone(
            sccache_strict.select_owned_mutable_release(
                [[self.release(), duplicate]],
                expected_id=None,
                tag=self.tag,
                head_sha=self.head_sha,
                ownership_marker=self.marker,
            )
        )

    def test_cleanup_tag_must_still_target_exact_head_commit(self) -> None:
        owned = {
            "ref": f"refs/tags/{self.tag}",
            "object": {"type": "commit", "sha": self.head_sha},
        }
        replaced = copy.deepcopy(owned)
        replaced["object"]["sha"] = "c" * 40

        sccache_strict.validate_cleanup_tag(
            owned,
            tag=self.tag,
            head_sha=self.head_sha,
        )
        with self.assertRaisesRegex(ValueError, "exact head commit"):
            sccache_strict.validate_cleanup_tag(
                replaced,
                tag=self.tag,
                head_sha=self.head_sha,
            )


class CliTests(unittest.TestCase):
    def test_show_target_emits_governed_json(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            result = sccache_strict.main(
                [
                    "show-target",
                    "--config",
                    str(REPO_ROOT / "ci/sccache-strict.toml"),
                    "--architecture",
                    "ARM64",
                ]
            )

        self.assertEqual(result, 0)
        output = json.loads(stdout.getvalue())
        self.assertEqual(output["source_version"], "0.16.0")
        self.assertEqual(output["target"], "aarch64-unknown-linux-musl")
        self.assertEqual(
            output["container"], f"docker.io/clux/muslrust@{CONTAINER_DIGEST}"
        )
        self.assertEqual(output["features"], ["s3", "vendored-openssl"])
        self.assertFalse(output["default_features"])
        self.assertEqual(output["profile"], "release")
        self.assertRegex(output["derivative_identity"], r"^[0-9a-f]{64}$")

    def test_invalid_architecture_returns_nonzero_without_traceback(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = sccache_strict.main(
                [
                    "show-target",
                    "--config",
                    str(REPO_ROOT / "ci/sccache-strict.toml"),
                    "--architecture",
                    "RISCV64",
                ]
            )

        self.assertEqual(result, 1)
        self.assertIn("architecture is not governed", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
