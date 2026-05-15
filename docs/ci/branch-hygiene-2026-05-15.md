# Branch Hygiene Inventory - 2026-05-15

Source commands: `gh api repos/seungpyoson/bolt-v2/branches --paginate`, `gh pr list --state all --limit 500`, `git branch -r --merged origin/main`.

No deletion was performed. Any branch deletion requires explicit approval.

| Branch | Head SHA | Classification | Rationale | Proposed action |
| --- | --- | --- | --- | --- |
| `001-v3-nucleus-admission` | `3f766973a16e8b056a062d0e5732c3bc2ae4783a` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `003-bolt-v3-strategy-registration` | `1ed1c3d0aee1becf2e73ae41103cdf0b27f7c071` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `004-bolt-v3-reference-capabilities` | `712750e1db8fe10bc314d95dbc16547c1147768a` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `005-bolt-v3-chainlink-provider-binding` | `64f0d0cb15d85142740c2a7f60614bd4a0d2ef86` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `006-bolt-v3-reference-role-validation` | `0d668426ee4ad41de0196f1523fb3d3fa5134626` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `007-bolt-v3-strategy-runtime-mapping` | `758ece8b169e5e5baf8c4694a08f820d37e4a1b6` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `008-bolt-v3-live-node-strategy-registration` | `928c1e77f774a03e1a5291fdcb87f9e14c26cf21` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `009-bolt-v3-phase5-decision-evidence-plan` | `0170ef0fbc370bc4197d455aeff960bb1681a972` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `010-bolt-v3-phase5-decision-evidence` | `dca20bb6a798801c99db16329200d1e70db42909` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `011-bolt-v3-phase6-submit-admission-plan` | `9672714b71d3bbdf044cf73872b2a5eba3635050` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `012-bolt-v3-phase6-submit-admission` | `d365bbbb78190653f67163fa61aecc4ad8d0d476` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `013-bolt-v3-phase7-no-submit-readiness-plan` | `462333d5fdc94289467b2220637036216d69e1e3` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `014-bolt-v3-phase7-no-submit-readiness` | `729acf795d2c1d0ae66753a78574adf77a25dc67` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `015-bolt-v3-phase8-tiny-canary-plan` | `b15f9f5549b7ffb4923bcb0fe8dfdbbdb621fb25` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `018-bolt-v3-phase8-canary-readiness-fresh` | `85344f909ae64fa91d8dd9f1fe88c498ac7c86d5` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `019-bolt-v3-phase9-audit-fresh` | `bce2d1e3d57c17cdc4bd3ca9f4c209a4de2ad75f` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `022-bolt-v3-phase9-current-main-audit` | `a4715c4845c39bc100d298cbed36e16abfdd5702` | active | open PR #331 (draft) | keep |
| `codex/bolt-v3-chainlink-provider` | `1a967caf046893deb6320569bbcf9330f30c9ce5` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-cross-strategy-capacity` | `83ae2a1f224ca4f57c343070a561f12e1462bd3a` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-decision-event-context` | `9eed2418fa79cd1e7ba10dd801b9a8ecced69d47` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-decision-event-contract` | `a1c79895db4c7e064f440591855e37c46d87cae8` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-decision-event-handoff` | `85d0f09ce87d0ae75cf0650d0c4083ae877e0a7b` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-decision-events` | `efa289caf7cb9f2f11eae5098b08d0434b3d3c1c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-capacity-accounting` | `3d2dfd7b18c1ab6b2d8f27208051852e85e4e20c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-fair-prob-no-action` | `db5bb2c81782333311d76df1a98ba963a4931c0b` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-fee-no-action` | `fadd790aeeaac02233951716f802d051129a4a74` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-gate-no-action-evidence` | `8ba1daf6ebe86b90815b74d24da0e9e33407864a` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-invalid-quantity-rejection` | `17361d245895da1b7afd1622845141e89a73bf27` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-market-ended-no-action` | `1b04e4dbbbc0d237160be930b09e08c54bd6afba` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-market-not-started-no-action` | `e64aba542c5323971c3485677a14218e0f85fb8e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-no-action-evidence` | `19368b2455db75c7fdfbd017d669bdc2f4653f6e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-open-orders-no-action` | `b3a1e65445b23b89946ca3ad07aa77e3ad413795` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-position-limit-no-action` | `efa0c3d18065a7d6f3177263151a0f3cc4b04d4e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-pricing-no-action` | `4b1ba48f0f6fc607ac01ac11f3b9f846dea2fa3d` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-rejection-coverage` | `0615f672a3a1101a2afd040c6db5d3df24f4c8d9` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-sizing-admission` | `ef66928fda00a1468aa3ac231f47abf317af7e9c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-entry-stale-reference-no-action` | `cd95df56ac9b5534f82f5249ba9a63e7e60e81bc` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-evaluation-decision-events` | `c6dc5d64f9ff8e09808640fa69ff6e5e7d8161c1` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-evidence-ledger` | `5d6f78566711bfa67645f65789fd5b0183ae1d80` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-bid-unavailable-evaluation` | `b93794f5920c783ff446435703406b43b00d166f` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-covered-evaluation` | `ec5f6f2bbe801cf5833da12587d547a2a2d285cb` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-evaluation-evidence` | `1d2dda592ec665af5791fad64350f473696b556c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-evaluation-reasons` | `4d64f2a26abdf455221af65980521e46332f4b75` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-evidence-failure-blocks-submit` | `88d02bcdbc289e1be9542ea88684d87409a461c1` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-invalid-evaluation` | `6feb3a9c8694b85bb3dca14b1d99e2a17f348a92` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-invalid-quantity-rejection` | `642e123f8eaf47418aca412026509c9b8bfd785c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-partial-sellable-sizing` | `9df520a685ad754bf15bf21da254b43f8717a444` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-rejection-evidence` | `a49346f2452017f736add3ea91ae54b72f09c5c2` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-exit-sellable-rejection` | `e021941852a1aee173890edec5f890dd9983f388` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-f7-reason-ownership` | `ac3146b8ad2dda1806ff8cb31fc73ce639c92be2` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-fast-venue-no-action` | `f9af42181e4b28ebad9b9828916954023ceb8c9c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-filled-position-capacity` | `3f5c09c55580482a64da875a2f4e05be7df5ec4a` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-forced-flat-entry-no-action` | `a86b80630b0ba1b8e80f1d271200fcd2f7953c0e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-freeze-entry-no-action` | `8ffe9baed2462c875c0dc7218e5d8086826219a4` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-fused-price-policy` | `e06ec77c892b6fc6a23ceb8fca361318cd27c86d` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-instrument-gate` | `b0d83d0c0ac48bf81f605ee2b89a7cce03949b2e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-instrument-load` | `2b65b06c0761850bf874a3153209608022919046` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-instrument-readiness` | `c6a0bfbddfa09f22d52f1f15201477de91a001ee` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-live-canary-contract` | `ee06dfbd43b87af0b948ca034ad89687175580b4` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-market-selection-event` | `96078a22fe395e3d59b1927ce3af8d6011cb1113` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-market-selection-facts` | `ee2ae6003bcd998210cda84e45d3141d51ebcb9e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-market-selection-failure-event` | `fad79386973e8d8dc59bd55f0ae07430ab2d357f` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-market-selection-failure-reasons` | `02342e563adc0208042366f1c3064d85b9bdcc51` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-one-position-no-action` | `ee07d8b6d45a7e85ff0b040ac202a957e21259e8` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-order-decision-events` | `93a6d5da1efc1e859795e63c6a98f7ee38c581ea` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-order-intent-gate` | `d2faebb79c6516e539ce0a23f0ff25d4dc3d514b` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-order-lifecycle` | `224caa7ef53b7fb16b1ed90df54602adfdaf007c` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-order-lifecycle-proof` | `a0f78627aa05f2f5fda0fa48e6c3ef917eb4694b` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-persistence-failed-latch` | `85be3fcad8f56340bffc86b7516c0168fecb7dc2` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-polymarket-no-submit-readiness` | `fde4ab74d7ec9be58aacf0fcd5ea8a347a1cc2e5` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-pre-submit-reason-allowlist` | `6c892a04f00a07d8f7909a82f89727b8eebab837` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-pre-submit-rejection-evidence` | `81672376e6859b6eb4792a7938fb4dbb608f3fc4` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reconciliation` | `42e4ba18eda6d5cfda925415f6c169ba9f4124bf` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reconciliation-crash-restart` | `b983b7fb2a947b80a770887c718bbf449d8d1d31` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reference-actor-registration` | `9606a729df62961ec1be19e62ab3e53223f1efbd` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reference-contract` | `bb30ec247a06daf16537200db5d396eceb4c67dc` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reference-delivery-proof` | `571e6ae9056841ca5ce417e3b796097740d3106f` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reference-facts` | `d68dd9ebbec8a6a34650190ca786e94c2370c65e` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-reference-producer` | `2551d712bfbad13bb8728f57c050679e0c307910` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-release-identity` | `429d9fbd60641bbe7643c181cf3333320b88659b` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-risk-admission-policy` | `1a2fefb83871061f2d9adc963c17578eb872e2a3` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-risk-exposure-no-action` | `322321ae045853ae26ef61f9819ebf822fa69ae8` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-risk-gate` | `9fd384f0b5469bcdae94034ce910c4114ae7869d` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-risk-order-admission` | `f6f8d4ebb622a0030c0e70f8f75fe8489911175f` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-risk-trading-state-config` | `8fd5b468903a786b73648c6342ecd4145eb70b24` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-slice-3-10-audit` | `249e7b2f7e0f507359f8f4accb4bfd04530b1184` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-slice-3-10-completion-audit` | `89a137d62d1578b83190644ee1f49fef12fc20c8` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-start-readiness-gate` | `a69ecc580e05017571806322ae7d33edf5503cd0` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-strategy-decision-events` | `fccedee9b5e3a2656a557b8a4d5d22f323535083` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-strategy-idle` | `936978bd479690d6219c0b43df763b1767e05bd3` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-strategy-order-intent-wiring` | `8b403eb920a8e094f2c83dcf4dfda4e2de47ab44` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/bolt-v3-trading-state-evidence` | `ac6cbf8d95148a21905d79ac2d0a6080bc707c71` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |
| `codex/ci-195-nextest-artifact-cache` | `ef20f4fc453e5c907b840dc6a58997e77fac932f` | active | open PR #349 (draft) | keep |
| `codex/ci-203-workflow-hygiene` | `85e6fb722adddaf28e6cda122154b60be973ba09` | active | open PR #347 (ready) | keep |
| `codex/ci-205-same-sha-smoke-dedup` | `7ebd8574126c6b8af4c5e5a1b8d19ae97caba4cf` | active | open PR #350 (draft) | keep |
| `codex/ci-332-parallel-heavy-lanes` | `e9028e2e150d88a324a7311f8237d9c7ea6ce01e` | active | open PR #348 (draft) | keep |
| `codex/ci-333-baseline` | `f961303ef0ff995064b1536cf2593dd6ce6f21fd` | active | open PR #345 (ready) | keep |
| `codex/ci-342-source-fence` | `3d1c49ecbc5ea32c54a0dfb1fb8f1055e0b3ab15` | active | open PR #346 (ready) | keep |
| `codex/ci-344-residual-minute-work` | `c6a2b23973898ffc12a6bd45147c755e97384f41` | active | open PR #351 (draft) | keep |
| `codex/governance-proof-tooling-lane` | `115b5aeadae0577f82045d163348859f3172a213` | active | open PR #279 (draft) | keep |
| `codex/phase6-submit-admission-recovery` | `9dcde75c03e9f30a02a3aa59d416623a90c8f2eb` | reference-only | no open PR and not merged into origin/main | keep as reference unless owner approves deletion |

## Summary

- active: 9
- reference-only: 92
- dead-merged-prunable: 0
