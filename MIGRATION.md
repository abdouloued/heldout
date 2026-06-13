# Migration From AgentScope

`tattle` is a focused relaunch of the AgentScope core idea: hold coding agents accountable at the git boundary.

## Harvested

- Git diff boundary: kept the `git2` approach and reduced it to a small worktree diff reader.
- Config shape: kept YAML config with serde defaults, renamed to `tattle.yaml`.
- Session shape: kept the idea of recording the active mission, renamed state to `.tattle/session.json`.
- Reporting contract: kept terminal output, JSON output, and non-zero exit codes for CI.
- MCP surface: kept a minimal JSON-RPC server with a `tattle_check` tool.

## Dropped

- 5-mode TUI cockpit
- Chat mode
- Themes
- Sessions browser
- Launcher lab
- Skills and plugin marketplace commands
- Broad agent-detection matrix
- Deterministic path-blocking policy

## Smaller Interpretations

- The first implementation leads with deterministic C1-C6 checks and held-out replay.
- The judge configuration is documented but intentionally inactive.
- Replay commands are explicit user-configured shell commands, because test runners already live behind project-specific command lines.
