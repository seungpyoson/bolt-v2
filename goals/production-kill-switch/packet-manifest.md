# Production Kill Switch Review Packet Manifest

Manifest generated for the external model approval gate after the internal adversarial review hardening pass. External review imports should match this packet unless the design packet is intentionally revised and the gate is reset.

Repo HEAD when generated: `6ba549dcf15823484b5546f4eb314371479d7cc0`

Prompt SHA-256 from source-not-sent approval preflights:

- DeepSeek rendered prompt hash: `2c9b4cc322dc5a0ad66a4e1ac95426c3570cf901ee2ed73a68bc96e19cedbf31`
- GLM rendered prompt hash: `32405e2590f491db444be6320452ed94950e881c52d9878e23b3e33ae2b9504a`

Packet totals from DeepSeek/GLM preflight: 15 files, 299,176 bytes, 6,122 lines.

| Path | Lines | Bytes | SHA-256 |
| --- | ---: | ---: | --- |
| `goals/production-kill-switch/facts.md` | 24 | 4,218 | `bc937cfa36d63d1663030518e2606ba5e0fbbb7d201d140df6744be74b071633` |
| `goals/production-kill-switch/research.md` | 106 | 8,808 | `ffd5ad478d03eb2f82191f42679287939f662ae0bbb6190086e911eff2684701` |
| `goals/production-kill-switch/design.md` | 237 | 16,113 | `9d99e53a3d452d6aa881fa090ffd4557d1287e5954f8ca879a7d8b757771cc87` |
| `goals/production-kill-switch/plan.md` | 87 | 6,703 | `5f9c9e5bed100be9dd62ced31ccf9bcd743d7924f16e7262bdb8136297c88633` |
| `goals/production-kill-switch/review-packet.md` | 69 | 3,765 | `82fd69793f7d52635f2cccb73fb7246bf3ff1d894623afa0309dd59c1596e132` |
| `goals/production-kill-switch/source-excerpts/binary-oracle-edge-taker-submit-path.md` | 164 | 5,114 | `367fe6b75835fd1d660c25a6ef1069c049e372352e1ae6dd739f1cec10b029a5` |
| `specs/505-nt-loss-governor/spec.md` | 110 | 9,071 | `4377564e18fe78efe145bd06f9cb8a5ed0be1bb276502b288a0abc126392a598` |
| `specs/505-nt-loss-governor/plan.md` | 97 | 4,718 | `78f01228be09417205c41a088c915ccbfc3a77bf0365388aa9a54c75b5c1b7f4` |
| `src/bolt_v3_submit_admission.rs` | 388 | 13,542 | `800c94bc58d3f21114cb9f47da41e256b5234f419564905ef09e3df4cdff78f4` |
| `src/bolt_v3_live_node.rs` | 3,540 | 140,704 | `ff81ecb1ea924d0b6ec55536e78699e3d1bb743d70d5c4a37aeabe298336bbbe` |
| `src/bolt_v3_strategy_registration.rs` | 135 | 4,570 | `dbd5fd6ba89670ea3490b8b987114f9bdf1e175eae83f11e127a8bb88a57d0ce` |
| `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` | 149 | 32,039 | `077cf8f81bbdd2b781ada3eee95538fc0757553cc799c4eb4f74576a648a9022` |
| `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml` | 831 | 43,384 | `c1d85831849f4fb77f95ff216311f2cd537bc437a781e1ce2f0552d8b5fe3afb` |
| `scripts/verify_bolt_v3_strategy_policy_fence.py` | 126 | 3,410 | `0cacca9f4d7c5f8ecd1f342f664d0c6c9cf7b5ec500709a231bb9352cde6e8d6` |
| `Cargo.toml` | 59 | 3,017 | `833f16cf31bcbe2ebad57496beb60af97714bbb9dbd0db07a991aad02ebb3727` |
