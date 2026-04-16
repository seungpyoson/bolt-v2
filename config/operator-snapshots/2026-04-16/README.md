Approved local operator-config snapshot captured on 2026-04-16.

- Source file: `config/live.local.toml`
- Purpose: durable dated copy of the reviewed local operator config so it survives deploy worktree cleanup
- Code branch carrying the anchor fix: `wip/eth-anchor-source-fix`
- Code commit at capture time: `9b2a449`

Notes:
- This snapshot is a copy of the local operator config at capture time.
- It is not the generated runtime artifact.
- Secrets remain as SSM parameter paths only.
- The active local operator file remains `config/live.local.toml`.
