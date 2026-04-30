# Graphify Repo Integration

## Purpose

Graphify is enabled as repo navigation tooling. It can build a knowledge graph from source/docs and remind agents to consult that graph before broad architecture search.

## Tracked Surface

- `.codex/hooks.json`
- `.claude/settings.json`
- `.gemini/settings.json`
- `.cursor/rules/graphify.mdc`
- `.kiro/steering/graphify.md`
- `.opencode/plugins/graphify.js`
- `opencode.json`

These files are hook/config surfaces only. They do not change build, runtime, CI, trading, or deployment behavior.

## Generated Output Policy

`graphify-out/` is ignored.

Generated graph artifacts can be useful local evidence, but they should not be committed by default. If a graph output needs to be tracked as evidence, that must be an explicit PR scope with the corpus, command, and reason stated.

## Source-Of-Truth Rule

Graphify output is a navigation aid, not product truth. Architecture claims still require source-file, test, CI, or documented contract evidence.
