#!/usr/bin/env python3
"""Regression tests for the closed #1354 evidence registry verifier."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER = ROOT / "scripts/verify_bolt_v3_evidence_registry.py"
REGISTRY = ROOT / "ci/bolt-v3-evidence-registry.toml"


class EvidenceRegistryVerifierTests(unittest.TestCase):
    def run_verifier(self, registry_text: str | None = None) -> subprocess.CompletedProcess[str]:
        if registry_text is None:
            path = REGISTRY
            temp = None
        else:
            temp = tempfile.TemporaryDirectory()
            path = pathlib.Path(temp.name) / "registry.toml"
            path.write_text(registry_text, encoding="utf-8")
        try:
            return subprocess.run(
                ["python3", str(VERIFIER), "--registry", str(path)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
        finally:
            if temp is not None:
                temp.cleanup()

    def run_with_source_mutation(
        self, relative_path: str, old: str, new: str
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            shutil.copytree(ROOT / "src", temp_root / "src")
            shutil.copytree(
                ROOT / "scripts",
                temp_root / "scripts",
                ignore=shutil.ignore_patterns("test_*", "__pycache__"),
            )
            path = temp_root / relative_path
            source = path.read_text(encoding="utf-8")
            mutated = source.replace(old, new, 1)
            self.assertNotEqual(source, mutated)
            path.write_text(mutated, encoding="utf-8")
            return subprocess.run(
                [
                    "python3",
                    str(VERIFIER),
                    "--registry",
                    str(REGISTRY),
                    "--root",
                    str(temp_root),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

    def run_with_added_file(
        self, relative_path: str, content: str
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            shutil.copytree(ROOT / "src", temp_root / "src")
            shutil.copytree(
                ROOT / "scripts",
                temp_root / "scripts",
                ignore=shutil.ignore_patterns("test_*", "__pycache__"),
            )
            path = temp_root / relative_path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
            return subprocess.run(
                [
                    "python3",
                    str(VERIFIER),
                    "--registry",
                    str(REGISTRY),
                    "--root",
                    str(temp_root),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

    def test_repository_registry_and_source_census_are_complete(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("evidence registry verified", result.stdout)

    def test_unknown_row_field_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'schema_version = 1\n',
            'schema_version = 1\nunknown_authority = "forbidden"\n',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown registry keys", result.stderr)

    def test_unknown_producer_row_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'method = "record_order_intent"',
            'method = "record_unknown_evidence"',
            1,
        )
        self.assertNotEqual(source, mutated)
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("callsite method mismatch", result.stderr)

    def test_new_unregistered_producer_callsite_is_rejected(self) -> None:
        call = ".record_entry_skip(&evidence)"
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs",
            call,
            call + "\n            .record_entry_skip(&evidence)",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer callsite census mismatch", result.stderr)

    def test_multiline_unregistered_producer_call_is_rejected(self) -> None:
        call = ".record_entry_skip(&evidence)"
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs",
            call,
            call + "\n            .\n            record_entry_skip\n            (&evidence)",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer structural authority", result.stderr)

    def test_unregistered_ufcs_producer_call_is_rejected(self) -> None:
        call = ".record_entry_skip(&evidence)"
        ufcs = (
            "BoltV3DecisionEvidenceWriter::record_entry_skip("
            "self.context.decision_evidence().as_ref(), &evidence);\n        "
        )
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", call, ufcs + call
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer callsite census mismatch", result.stderr)

    def test_method_alias_and_function_pointer_dispatch_are_rejected(self) -> None:
        call = ".record_entry_skip(&evidence)"
        alias = (
            "let emit = BoltV3DecisionEvidenceWriter::record_entry_skip;\n"
            "        emit(self.context.decision_evidence().as_ref(), &evidence);\n        "
        )
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", call, alias + call
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer structural authority", result.stderr)

    def test_module_level_method_alias_is_rejected(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        alias = "const EMIT_ENTRY_SKIP: usize = BoltV3DecisionEvidenceWriter::record_entry_skip as usize;\n\n"
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, alias + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer structural authority", result.stderr)

    def test_wrapper_dispatch_is_rejected(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        wrapper = """
fn emit_entry_skip_wrapper(
    writer: &dyn BoltV3DecisionEvidenceWriter,
    evidence: &BoltV3EntrySkipEvidence,
) {
    let emit = BoltV3DecisionEvidenceWriter::record_entry_skip;
    emit(writer, evidence);
}

"""
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, wrapper + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer structural authority", result.stderr)

    def test_macro_dispatch_is_rejected(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        wrapper = """
macro_rules! emit_evidence {
    ($writer:expr, $method:ident, $value:expr) => {
        $writer.$method($value)
    };
}

fn emit_entry_skip_macro(
    writer: &dyn BoltV3DecisionEvidenceWriter,
    evidence: &BoltV3EntrySkipEvidence,
) {
    emit_evidence!(writer, record_entry_skip, evidence);
}

"""
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, wrapper + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer structural authority", result.stderr)

    def test_mispointed_producer_callsite_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace("::record_entry_skip_once::", "::moved_entry_skip::", 1)
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer callsite census mismatch", result.stderr)

    def test_duplicate_producer_callsite_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        call = (
            '"src/strategies/binary_oracle_maker/mod.rs::update_requote_throttle_edge::'
            'record_requote_throttle::1"'
        )
        mutated = source.replace(f"call_sites = [{call}]", f"call_sites = [{call}, {call}]", 1)
        self.assertNotEqual(source, mutated)
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate callsite", result.stderr)

    def test_snapshot_structural_split_cannot_be_swapped(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        submit = "::submit_admitted_entry_decision::"
        blocked = "::record_blocked_entry_strategy_input_snapshot_once::"
        mutated = source.replace(submit, "::SWAP::", 1).replace(blocked, submit, 1).replace(
            "::SWAP::", blocked, 1
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("submit snapshot row", result.stderr)

    def test_new_unregistered_reader_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            "fn read_jsonl_lines(path: &Path)",
            "fn read_unregistered_evidence(path: &Path) -> Result<Vec<String>> { read_decision_evidence_jsonl_lines(path) }\n\nfn read_jsonl_lines(path: &Path)",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reader structural authority", result.stderr)

    def test_renamed_load_reader_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path)"
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            "fn load_evidence(path: &Path) -> Result<Vec<(usize, String)>> { read_jsonl_lines(path) }\n\n"
            + marker,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reader structural authority", result.stderr)

    def test_async_reader_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path)"
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            "async fn consume_evidence(path: &Path) -> Result<Vec<(usize, String)>> { read_jsonl_lines(path) }\n\n"
            + marker,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reader structural authority", result.stderr)

    def test_passed_path_helper_indirection_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        helper = """fn consume_passed_path(path: &Path) -> Result<Vec<String>> {
    let file = File::open(path)?;
    Ok(std::io::BufRead::lines(std::io::BufReader::new(file)).collect::<std::io::Result<Vec<_>>>()?)
}

"""
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            helper
            + "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    consume_passed_path(path)?",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence I/O authority", result.stderr)

    def test_aliased_fs_reader_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        helper = """fn consume_aliased_fs(path: &Path) -> Result<Vec<String>> {
    use std::fs as disk;
    let payload = String::from_utf8(disk::read(path)?)?;
    Ok(payload.lines().map(str::to_owned).collect())
}

"""
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            helper
            + "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    consume_aliased_fs(path)?",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence I/O authority", result.stderr)

    def test_file_options_reader_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        helper = """fn consume_file_options(path: &Path) -> Result<Vec<String>> {
    use std::io::Read;
    let mut file = File::options().read(true).open(path)?;
    let mut payload = String::new();
    file.read_to_string(&mut payload)?;
    Ok(payload.lines().map(str::to_owned).collect())
}

"""
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            helper
            + "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    consume_file_options(path)?",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence I/O authority", result.stderr)

    def test_write_macros_are_rejected_outside_authority(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        helper = """
fn rewrite_evidence(path: &Path) -> Result<()> {
    let mut file = File::options().write(true).open(path)?;
    write!(file, "{}", "record")?;
    writeln!(file, "{}", "record")?;
    Ok(())
}

"""
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            helper
            + "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    rewrite_evidence(path)?;\n    read_decision_evidence_jsonl_lines(path)?",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence I/O authority", result.stderr)

    def test_serde_writer_is_rejected_outside_authority(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        helper = """
fn serialize_evidence(path: &Path, value: &serde_json::Value) -> Result<()> {
    let file = File::options().write(true).open(path)?;
    serde_json::to_writer(file, value)?;
    Ok(())
}

"""
        result = self.run_with_source_mutation(
            "src/shadow_pnl.rs",
            marker,
            helper
            + "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    serialize_evidence(path, &serde_json::Value::Null)?;\n    read_decision_evidence_jsonl_lines(path)?",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence I/O authority", result.stderr)

    def test_new_python_reader_is_rejected(self) -> None:
        result = self.run_with_added_file(
            "scripts/new_evidence_tool.py",
            """from pathlib import Path

def load_records(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines()

DECISION_EVIDENCE_KIND = "strategy_input_snapshot"
""",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python raw-I/O closed census mismatch", result.stderr)

    def test_new_rust_raw_reader_is_rejected_by_whole_tree_census(self) -> None:
        result = self.run_with_added_file(
            "src/new_evidence_reader.rs",
            """use std::{fs::File, io::Read, path::Path};

fn load_records(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut payload = String::new();
    file.read_to_string(&mut payload)?;
    Ok(payload)
}
""",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("whole-tree raw-I/O census mismatch", result.stderr)

    def test_rust_raw_io_function_pointer_is_rejected(self) -> None:
        result = self.run_with_added_file(
            "src/innocent_alias.rs",
            """use std::path::Path;

fn load(path: &Path) {
    let reader = std::fs::read;
    let _ = reader(path);
}
""",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("whole-tree raw-I/O census mismatch", result.stderr)

    def test_rust_module_macro_with_raw_io_is_rejected(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        injected = """macro_rules! hidden_read {
    ($path:expr) => { std::fs::read($path) };
}

"""
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, injected + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("raw I/O in module macro", result.stderr)

    def test_rust_reader_cross_module_raw_helper_is_rejected(self) -> None:
        marker = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    read_decision_evidence_jsonl_lines(path)?"
        replacement = "fn read_jsonl_lines(path: &Path) -> Result<Vec<(usize, String)>> {\n    let _ = read_file_bounded(path, 1);\n    read_decision_evidence_jsonl_lines(path)?"
        result = self.run_with_source_mutation("src/shadow_pnl.rs", marker, replacement)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Rust evidence reader reaches unregistered raw-I/O helper", result.stderr)

    def test_reader_root_list_shrinkage_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'reader_census_roots = ["src", "scripts"]',
            'reader_census_roots = ["src"]',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reader census must cover src and scripts", result.stderr)

    def test_producer_root_list_shrinkage_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'producer_census_roots = ["src"]',
            'producer_census_roots = ["src/strategies"]',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer census must cover the complete src tree", result.stderr)

    def test_handler_sweep_roots_cannot_be_emptied_or_shrunk(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        for replacement in ('handler_sweep_roots = []', 'handler_sweep_roots = ["src/strategies"]'):
            with self.subTest(replacement=replacement):
                mutated = source.replace(
                    'handler_sweep_roots = ["src/strategies", "src/bolt_v3_live_node.rs", "src/bolt_v3_live_node"]',
                    replacement,
                    1,
                )
                result = self.run_verifier(mutated)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("handler_sweep_roots", result.stderr)

    def test_cfg_not_test_producer_is_counted_as_production(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        injected = """#[cfg(not(test))]
fn production_only_alias(writer: &dyn BoltV3DecisionEvidenceWriter, evidence: &BoltV3EntrySkipEvidence) {
    writer.record_entry_skip(evidence);
}

"""
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, injected + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer callsite census mismatch", result.stderr)

    def test_module_level_macro_with_exact_record_method_is_rejected(self) -> None:
        marker = "impl BinaryOracleEdgeTaker {"
        injected = """macro_rules! hidden_record {
    ($writer:expr, $value:expr) => { $writer.record_entry_skip($value) };
}

"""
        result = self.run_with_source_mutation(
            "src/strategies/binary_oracle_edge_taker/mod.rs", marker, injected + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("macro_or_out_of_function_record_call", result.stderr)

    def test_reader_registry_shrink_from_twenty_to_seventeen_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        start = source.index("[[reader]]")
        producer = source.index("[[producer]]")
        reader_text = source[start:producer]
        blocks = reader_text.split("[[reader]]")[1:]
        self.assertEqual(len(blocks), 20)
        mutated = source[:start] + "".join(
            "[[reader]]" + block for block in blocks[:17]
        ) + source[producer:]
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly 20 rows", result.stderr)

    def test_python_path_open_without_marker_is_rejected(self) -> None:
        result = self.run_with_added_file(
            "scripts/innocent_name.py",
            "from pathlib import Path\n\ndef load(path: Path):\n    with path.open() as handle:\n        return handle.read()\n",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python raw-I/O closed census mismatch", result.stderr)

    def test_python_read_text_function_pointer_without_marker_is_rejected(self) -> None:
        result = self.run_with_added_file(
            "scripts/innocent_alias.py",
            "from pathlib import Path\n\ndef load(path: Path):\n    reader = path.read_text\n    return reader()\n",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python raw-I/O closed census mismatch", result.stderr)

    def test_python_reader_cross_module_raw_helper_is_rejected(self) -> None:
        marker = "def migrate_file_bytes(path: Path, payload: bytes) -> bytes:\n"
        replacement = (
            marker
            + "    require_text_file(path.parent, path, [])\n"
        )
        result = self.run_with_source_mutation(
            "scripts/migrate_bolt_v3_decision_evidence_to_v15.py", marker, replacement
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python evidence reader reaches unregistered raw-I/O helper", result.stderr)

    def test_evidence_authority_is_not_wholesale_exempt_from_raw_io_census(self) -> None:
        marker = "fn serialize_optional_local_receive_ms"
        injected = "fn unregistered_authority_reader(path: &Path) { let _ = std::fs::read(path); }\n\n"
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs", marker, injected + marker
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("whole-tree raw-I/O census mismatch", result.stderr)

    def test_mispointed_reader_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'symbol = "read_settlements"', 'symbol = "read_settlements_moved"', 1
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reader census mismatch", result.stderr)

    def test_duplicate_reader_path_symbol_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'name = "shadow_pnl_read_settlements"\npath = "src/shadow_pnl.rs"\nsymbol = "read_settlements"'
        replacement = 'name = "shadow_pnl_read_settlements"\npath = "src/shadow_pnl.rs"\nsymbol = "read_admitted_entry_chains"'
        self.assertIn(marker, source)
        result = self.run_verifier(source.replace(marker, replacement, 1))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate path/symbol", result.stderr)

    def test_duplicate_family_id_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        first = source.index("[[producer]]")
        second = source.index("[[producer]]", first + 1)
        first_row = source[first:second]
        family = next(line for line in first_row.splitlines() if line.startswith("family = "))
        state_id = next(line for line in first_row.splitlines() if line.startswith("state_id = "))
        tail = source[second:]
        tail = tail.replace(
            next(line for line in tail.splitlines() if line.startswith("family = ")),
            family,
            1,
        ).replace(
            next(line for line in tail.splitlines() if line.startswith("state_id = ")),
            state_id,
            1,
        )
        result = self.run_verifier(source[:second] + tail)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate family/id", result.stderr)

    def test_recovery_bearing_suppression_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'recovery_bearing = true\nsuppression = "unsuppressed"'
        self.assertIn(marker, source)
        mutated = source.replace(
            marker,
            'recovery_bearing = true\nsuppression = "finite-monotone-mask"',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("recovery-bearing producer", result.stderr)

    def test_non_recovery_unsuppressed_row_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'recovery_bearing = false\nsuppression = "finite-monotone-mask"'
        self.assertIn(marker, source)
        result = self.run_verifier(
            source.replace(
                marker,
                'recovery_bearing = false\nsuppression = "unsuppressed"',
                1,
            )
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires finite monotone suppression", result.stderr)

    def test_suppressed_row_must_be_non_authoritative_diagnostic(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'authority_semantics = "non-authoritative-diagnostic"'
        self.assertIn(marker, source)
        result = self.run_verifier(
            source.replace(marker, 'authority_semantics = "authoritative"', 1)
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a non-authoritative diagnostic", result.stderr)

    def test_suppressed_row_consumers_must_tolerate_under_emission(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = (
            'under_emission_semantics = '
            '"consumer-tolerates-conservative-under-emission"'
        )
        self.assertIn(marker, source)
        result = self.run_verifier(
            source.replace(marker, 'under_emission_semantics = "forbidden"', 1)
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must tolerate conservative under-emission", result.stderr)

    def test_recovery_bearing_row_must_remain_authoritative(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'authority_semantics = "authoritative"'
        self.assertIn(marker, source)
        result = self.run_verifier(
            source.replace(
                marker, 'authority_semantics = "non-authoritative-diagnostic"', 1
            )
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must remain authoritative", result.stderr)

    def test_identity_type_excludes_forbidden_volatile_fields(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        identity_source = (ROOT / "src/bolt_v3_evidence_identity.rs").read_text(
            encoding="utf-8"
        )
        episode_struct = identity_source.split("pub struct EvidenceEpisodeId", 1)[1].split(
            "}", 1
        )[0]
        for forbidden in (
            "price",
            "timestamp",
            "slug",
            "window",
            "diagnostic",
            "retry",
            "schema",
            "config",
            "deployment",
            "order_id",
        ):
            self.assertNotIn(forbidden, episode_struct.lower())

    def test_nested_identity_type_rejects_forbidden_volatile_field(self) -> None:
        identity = (ROOT / "src/bolt_v3_evidence_identity.rs").read_text(
            encoding="utf-8"
        )
        mutated_identity = identity.replace(
            "    condition_id: NonEmptyEvidenceIdentity,",
            "    condition_id: NonEmptyEvidenceIdentity,\n    observed_price: u64,",
            1,
        )
        self.assertNotEqual(identity, mutated_identity)
        with tempfile.TemporaryDirectory() as temp_dir:
            identity_path = pathlib.Path(temp_dir) / "identity.rs"
            identity_path.write_text(mutated_identity, encoding="utf-8")
            registry = REGISTRY.read_text(encoding="utf-8").replace(
                'identity_module = "src/bolt_v3_evidence_identity.rs"',
                f'identity_module = "{identity_path}"',
                1,
            )
            result = self.run_verifier(registry)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden volatile fields", result.stderr)

    def test_alternate_identity_constructor_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/lib.rs",
            "pub mod bolt_v3_evidence_identity;",
            "pub mod bolt_v3_evidence_identity;\nconst _: &str = \"EvidenceEpisodeId::new\";",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("alternate construction", result.stderr)

    def test_unbounded_edge_mask_shape_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "static ENTRY_SKIP_NOVELTY: AtomicU16",
            "static ENTRY_SKIP_NOVELTY: AtomicU32",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing exact production mask", result.stderr)

    def test_dynamic_maker_mask_shape_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "static REQUOTE_THROTTLE_NOVELTY: AtomicU16",
            "static REQUOTE_THROTTLE_NOVELTY: Vec<AtomicU16>",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing exact production mask", result.stderr)

    def test_parallel_guard_history_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "static ENTRY_SKIP_NOVELTY: AtomicU16 = AtomicU16::new(0);",
            "static ENTRY_SKIP_NOVELTY: AtomicU16 = AtomicU16::new(0);\nstatic ENTRY_SKIP_HISTORY_NOVELTY: AtomicU16 = AtomicU16::new(0);",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("novelty static census mismatch", result.stderr)

    def test_nested_guard_collection_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "static REQUOTE_THROTTLE_NOVELTY: AtomicU16 = AtomicU16::new(0);",
            "static REQUOTE_THROTTLE_NOVELTY: AtomicU16 = AtomicU16::new(0);\nstatic REQUOTE_ARCHIVE_NOVELTY: AtomicU16 = AtomicU16::new(0);",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("novelty static census mismatch", result.stderr)

    def test_map_set_and_type_aliased_guard_histories_are_rejected(self) -> None:
        mutations = (
            "ENTRY_SKIP_BY_MARKET_NOVELTY",
            "ENTRY_SKIP_SET_NOVELTY",
            "ENTRY_SKIP_ARCHIVE_NOVELTY",
        )
        for field in mutations:
            with self.subTest(field=field):
                result = self.run_with_source_mutation(
                    "src/bolt_v3_decision_evidence.rs",
                    "static ENTRY_SKIP_NOVELTY: AtomicU16 = AtomicU16::new(0);",
                    f"static ENTRY_SKIP_NOVELTY: AtomicU16 = AtomicU16::new(0);\nstatic {field}: AtomicU16 = AtomicU16::new(0);",
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("novelty static census mismatch", result.stderr)

    def test_mask_reset_method_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "fn mark_u16_once(mask: &AtomicU16, index: u32, domain: u32) -> bool {",
            "fn mark_u16_once(mask: &AtomicU16, index: u32, domain: u32) -> bool {\n    mask.store(0, Ordering::Relaxed);",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must only change by fetch_or", result.stderr)

    def test_atomic_store_mutation_helper_is_rejected(self) -> None:
        result = self.run_with_source_mutation(
            "src/bolt_v3_decision_evidence.rs",
            "fn mark_u8_once(mask: &AtomicU8, index: u32, domain: u32) -> bool {",
            "fn mark_u8_once(mask: &AtomicU8, index: u32, domain: u32) -> bool {\n    mask.swap(0, Ordering::Relaxed);",
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must only change by fetch_or", result.stderr)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
