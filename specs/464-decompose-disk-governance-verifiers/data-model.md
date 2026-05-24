# Data Model: #464 Cargo Scanner Helper Decomposition

## CargoInvocationTokens

Ordered shell-token strings representing Cargo arguments after a client has already selected the relevant command segment.

Fields:

- `tokens: list[str]`: argument list passed to `cargo_subcommand_with_index`, `cargo_subcommand`, or `cargo_args_for_target_routing_scan`.
- `start: int`: optional scan offset used by the static verifier when the token list includes an executable before Cargo arguments.

Invariants:

- Tokens are already split by the caller's existing tokenization path.
- The shared helper does not tokenize strings.
- The shared helper does not inspect filesystem, environment, processes, Git state, network, credentials, or operator home paths.

## CargoSubcommandScan

The result of scanning Cargo arguments for the first non-global-option token.

Fields:

- `index: int`: index of the detected subcommand in the supplied token list.
- `subcommand: str`: detected subcommand token.

Invariants:

- Toolchain selectors beginning with `+` are skipped.
- Cargo global options with arguments are skipped with their value token.
- Cargo global options without arguments are skipped.
- Leading dash tokens not recognized by the option sets are skipped, preserving current client behavior.
- `--` before a subcommand is skipped, matching current behavior.

## NextestSubcommandScan

The result of scanning `cargo nextest` tail arguments for the `nextest` subcommand.

Fields:

- `index: int`: index of the detected nextest subcommand in the supplied nextest argument list.
- `subcommand: str`: detected nextest subcommand token.

Invariants:

- A `--` separator before a nextest subcommand means no nextest subcommand is detected.
- Nextest global options with argument values are skipped.
- Leading dash tokens are skipped.

## TargetRoutingScanTokens

Cargo arguments truncated to the policy-relevant prefix before binary/test arguments.

Fields:

- `tokens: list[str]`: Cargo arguments to scan for target-routing storage overrides.

Invariants:

- For `cargo bench`, `cargo run`, and `cargo test`, tokens after a post-subcommand `--` separator are excluded.
- For `cargo nextest run`, tokens after the post-`run` `--` separator are excluded.
- Other subcommands preserve the supplied token list.
- Detection of target-routing override options remains client-local.

## VerifierClient

A module that calls shared cargo scanner helpers while owning its own policy interface.

Clients:

- Runtime verifier: `scripts/rust_verification.py`
- Static workflow verifier: `scripts/verify_ci_workflow_hygiene.py`

Invariants:

- Runtime verifier keeps refusal payload shape and option string reporting local.
- Static verifier keeps workflow raw-storage and environment-prefix policy local.
- Both clients use the shared cargo scanner helper family for selected pure scanning behavior.
