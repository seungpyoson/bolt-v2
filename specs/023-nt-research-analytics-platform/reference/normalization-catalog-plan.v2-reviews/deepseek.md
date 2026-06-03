Verdict: SHIP-WITH-CHANGES

The plan addresses every v1 finding with concrete structural changes, but two design-level gaps remain: (1) the `ConditionalCatalogWriter`’s content+transform-hash keying is incompatible with NT’s interval-keyed reader, and no adapter is specified; (2) the PromotionPackage + pointer CAS mechanism lacks a reader that resolves the pointer, so NT’s catalog could still list orphan objects. Without these, the canonical catalog cannot be safely queried by `BacktestNode`. The design otherwise is sound.

### Per‑finding resolution table

| Finding | Verdict | Evidence |
|---------|---------|----------|
| F1 | RESOLVED | §2 three-tier matrix + `nt_target` column maps every contract family to Tier A/B/C/instruments lane; replay claim scoped to the 9 `NautilusDataType` members (confirmed against pinned rev 6e059dc). |
| F2 | PARTIAL | §4.3 `ConditionalCatalogWriter` with `PutMode::Create` and content+transform-hash keys fixes the write side. However, §2.2/§3 requires using NT’s `ParquetDataCatalog` for reads, but the custom naming (`<start>_<end>__t-<hash>__c-<hash>.parquet`) is incompatible with NT’s `timestamps_to_filename` parser (catalog.rs:4175-4180). No reader integration is specified. |
| F3 | RESOLVED | §6.1 canonical four‑family set derived from `contractType` with fail‑loud guard; binding TOML reconciled; acceptance test mandated. Open verification of delivery `contractType` strings remains but does not invalidate the design. |
| F4 | RESOLVED | §7.1 OKX `order_book_400` classified as snapshots with explicit forbidden‑claim; evidence matrix downgraded; derivation rule named. |
| F5 | RESOLVED |[redacted_source_excerpt] accepted manifests; host corrected to[redacted_source_excerpt]TOML and[redacted_source_excerpt] |
| F6 | RESOLVED | §7.3 forbidden‑[redacted_source_excerpt]allowlist applie[redacted_source_excerpt]‑only families;[redacted_source_excerpt]barred from `event_time`; class‑level prevention. |
| F7 | RESOLVED | §6.2 single three‑valued enum `[redacted_source_excerpt][redacted_source_excerpt][redacted_source_excerpt]exhaustive[redacted_source_excerpt]and[redacted_source_excerpt] |
| F8 | RESOLVED |[redacted_source_excerpt][redacted_source_excerpt]`sp:<contract>:[redacted_source_excerpt]:v0-pending`);[redacted_source_excerpt]populated only w[redacted_source_excerpt]=accepted`;[redacted_source_excerpt]eliminated. |
| F9 | RESOLVED | §8 best‑[redacted_source_excerpt][redacted_source_excerpt] symbol‑[redacted_source_excerpt]‑declared[redacted_source_excerpt]; mechanism in place,[redacted_source_excerpt]pending. |
| F10 | PARTIAL | §9 cost projection methodology, ledger‑[redacted_source_excerpt]‑amplification analysis, HL requester‑pays pre‑stage defined. The actual[redacted_source_excerpt]and approval are deferred as a gate; section provides the skeleton but not the completed estimate. |
| F11 | RESOLVED | §11‑cred[redacted_source_excerpt]0.3 specifies scrub of[redacted_source_excerpt]+ IMDS, and a delta between SSM‑present and SSM‑absent writes. IMDS‑[redacted_source_excerpt]is host‑dependent (open decision 11.4), but the test design is crisp. |
| F12 | RESOLVED |[redacted_source_excerpt][redacted_source_excerpt];[redacted_source_excerpt] permanently verifies absence of `aws`/`cloud` features in the live resolution. Structurally isolates cloud. |
| F13 | RESOLVED | §7.4 explicit forbidden‑claim “no HL‑[redacted_source_excerpt]”; `trades` moved[redacted_source_excerpt];[redacted_source_excerpt]gated as a separate future task. |
| F14 | RESOLVED | §5.2 per‑family proof‑[redacted_source_excerpt]; synthetic‑[redacted_source_excerpt] (§5.2 rule 2, Phase 3); provider‑[redacted_source_excerpt] (rule 3); reordered phases S7→S8. If enforced, no un‑[redacted_source_excerpt]byte reaches[redacted_source_excerpt]|
| F15 | PARTIAL | §4.4[redacted_source_excerpt]replaces prefix re‑point. However, §3–4 still say NT’s catalog is the read mechanism, and that catalog lists its prefix directly (catalog.rs query logic). The pointer is not consumed by the catalog reader, so orphan objects from superseded promotions could still be returned. The promotion scheme[redacted_source_excerpt] reader integration. |

### New findings

1. **BLOCKER — NT catalog reader incompatible with content‑hash‑keyed paths**
  [redacted_source_excerpt](§4.3.3) emits keys like `<prefix[redacted_source_excerpt]NT’[redacted_source_excerpt]` expects interval‑keyed filenames[redacted_source_excerpt]catalog.rs:4175‑4180) and its listing/parsing routines will not recognise these files. Since the plan mandates using NT’s catalog for reads (§3, §4.3, §5.2),[redacted_source_excerpt] will be unreadable or silently return empty/incorrect data. **Must specify either (a) a custom catalog that resolves the promotion pointer and understands the hash‑suffixed naming, or (b) revert to standard NT naming with a different multi‑version strategy.**

2. **MODERATE — Promotion pointer not wired to any reader**
   §4.4.5 states “c[redacted_source_excerpt]”. The[redacted_source_excerpt]’s[redacted_source_excerpt]path call[redacted_source_excerpt]query_files`, which lists the S3 prefix directly; it has no knowledge of[redacted_source_excerpt]. Without an adapter that transforms the pointer into a file list, the catalog will serve whatever objects exist under the[redacted_source_excerpt]—including superseded or non‑promoted objects. This undermines the F15 guarantee.

3. **LOW — Symbol‑shape parser[redacted_source_excerpt]is a blocking gate without default fallback**
   The instrument‑universe plan (§8) requires an owner‑[redacted_source_excerpt]; until declared,[redacted_source_excerpt]is blocked. The plan provides no safe default or timeout, which may stall the entire pipeline indefinitely. (This is noted i[redacted_source_excerpt] but constitutes a procedural risk.)

### Design level concerns (decisions most likely to be wrong)

- **Rust[redacted_source_excerpt]choice is correct, but the plan assumes NT’s stock catalog can be used as a passive reader after fundamental key format changes.** The team will likely need to write a small adapter that implements NT’s `DataCatalog` trait, consuming the pointer snapshot and translating the custom filenames into interval‑keyed metadata that NT expects. This is non‑trivial and currently unspecified.

- **The promotion pointer CAS gives atomic promotion of the metadata view, but the physical S3 objects persist forever (create‑only).** Over many promotion cycles, the[redacted_source_excerpt] will[redacted_source_excerpt] objects that are no longer referenced. While they do not appear through the pointer, any process that lists the bucket (not using the pointer) will see them. If housekeeping is ever needed (e.g., cost, compliance), the plan provides no mechanism to safely delete old objects—and deletion in S3 is not atomic, risking data loss. This is a long‑term operational gap.

- **The[redacted_source_excerpt] relies on exact IMDS/VM configuration in the build environment; the plan acknowledges this as an open decision (§13.4).** If Phase 0 is executed in an environment where IMDS cannot be completely blocked (e.g., a shared CI runner), the control may produce a false positive (write succeeding via ambient IMDS even with invalid[redacted_source_excerpt]. This could erode confidence in the SSM‑only proof.
