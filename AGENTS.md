# bolt-v2 Agent Rules

## Authority

- Direct user instructions win unless they violate safety.
- `AGENTS.md` is the repository authority. Tool-specific adapters must defer to it.
- After a merge, `main` is authoritative. Do not continue work from superseded branches or worktrees.

## Non-Negotiable Invariants

- **NT FIRST:** inspect NautilusTrader APIs and source before designing or implementing anything. Reuse NT whenever it provides the capability; rebuilding or shadowing an NT capability in Bolt is rejected. Bolt may own only missing domain policy and the thinnest necessary bindings.
- **No hardcodes:** runtime IDs, quantities, timeouts, and selectable values come from TOML.
- **No dual paths:** one config format, secret source, build path, and runtime path for each capability. No fallback or compatibility routes.
- **No debt:** no TODOs, unpinned dependencies, or unfinished work presented as complete.
- **No credential display:** never print or expose secrets.
- **Pure Rust runtime:** no Python runtime layer, PyO3, maturin, or pip.
- **SSM only:** product and runtime credentials come from AWS SSM through `aws-sdk-ssm`. GitHub automation may use only GitHub's ephemeral token for GitHub operations.
- **Do not reference Bolt v1:** use NautilusTrader source from Cargo checkouts or GitHub.
- **NT first:** inspect and reuse NautilusTrader before adding any Bolt-owned abstraction. If NT already provides the capability, duplicating it is rejected; Bolt may add only missing domain policy or thin bindings.
- **Strategies produce intent only:** shared execution modules own admissibility, venue rules, sizing, rounding, and submission.
- **Chainlink Data Streams testnet is production** for the `price_to_beat` oracle.
- **Register provider boundaries:** every deploy or readiness input derived from provider runtime data must be covered by the authoritative boundary registry and source-fence evidence.

## Verification and Merge

- Verify before claiming completion. Use direct inspection, tests, static checks, source fences, remote evidence, or live artifacts appropriate to the risk.
- Do not run compile-heavy Rust verification locally by default. Use `just fmt` and `just preflight`; publish with `just sandbox-safe-push`.
- Rust Probe is diagnostic only and never authorizes merge or deployment.
- Queue only through `just merge-queue <pr>`. Never post the Mergify command manually or bypass controls with `gh pr merge --admin`.
- Merging requires approval from GitHub node ID `U_kgDOEZMFhA` and native code-owner review, stale-review dismissal, last-push approval, and resolved review threads.
