---
name: openproject
description: Manage OpenProject projects and work packages through API v3. Use when a user asks to inspect, create, update, comment on, or log time against OpenProject work packages.
metadata:
  short-description: Manage OpenProject work packages
---

# OpenProject

Use the bundled `openproject` CLI. It is portable and non-interactive by default; only `openproject auth login` prompts deliberately for local credential setup.

## Installation and setup

Check availability with `openproject --version`. If the executable is missing, explain that the platform installer downloads a release archive and verifies its SHA-256 checksum, then obtain approval before running `scripts/install.sh` on Linux/macOS or `scripts/install.ps1` on Windows.

Upgrade an existing executable with `openproject upgrade`, optionally followed by a version without the leading `v`. Use `openproject upgrade --dry-run --json` when the source or destination needs review. Rerunning the platform installer also detects and upgrades an existing executable. Obtain approval before either upgrade path because it downloads and replaces the local executable.

The public skill source is the repository root of `yungts97/openproject-skill`. The executable installation is separate because it is platform-specific. Users with a private GitLab mirror may set `OPENPROJECT_GITLAB_PROJECT`, optionally `OPENPROJECT_GITLAB_HOST`, and use their existing `glab` login.

Remove the executable with `openproject uninstall`. Use `--dry-run` first when the resolved executable path needs review. This preserves configuration and the separately installed Agent Skill; remove the skill through the agent or skill manager that installed it.

The user supplies their own OpenProject URL and API token. Do not ask them to paste a token into chat, print it, place it in command arguments, or write it to a configuration file. For an interactive local terminal, direct them to run `openproject auth login`; it validates the token and stores it in the system credential manager, or an existing initialized `pass` store when the system manager is unavailable.

For agents, CI, headless machines, and temporary sessions, direct them to set the token in their environment:

```bash
export OPENPROJECT_TOKEN="opapi-..."
openproject auth verify
```

## Configuration and project resolution

The host resolves from `--host`, `OPENPROJECT_URL`, project configuration, then global configuration.

The global configuration is `openproject/config.json` under the platform config directory: XDG config on Linux, Application Support on macOS, or AppData on Windows. It accepts only a non-secret `host`:

```json
{"host":"https://openproject.example.com"}
```

Project configuration is `.openproject.json` at the Git root and may set `host` plus `project_id` or the compatibility key `project`:

```json
{"host":"https://openproject.example.com","project_id":13}
```

Read repository guidance before external writes. Use an explicit `--project` when guidance supplies one. Otherwise the CLI uses project configuration, then an exact repository-name match. If resolution is missing or ambiguous, stop and ask the user; never select a project speculatively.

## Agent-friendly operation

- Prefer `--json` for reads and automation. Runtime failures use the JSON stderr shape `{"error":{"message":"..."}}` and a non-zero exit code.
- Use `--dry-run --json` to review the method, API path, and payload when a write target or payload needs confirmation.
- Only `auth login` prompts interactively. Do not infer that successful authentication authorizes a later write.
- Resolve status, type, project, and user names exactly, or use numeric IDs when ambiguity is possible.
- Run `openproject COMMAND --help` rather than guessing unsupported arguments.

## Commands

```bash
openproject auth login
openproject projects --json
openproject project --project 13 --json
openproject tasks --project 13 --assignee me --query approval --json
openproject task 123 --json
openproject create --project 13 --subject "Fix approval flow" --type Task --assignee me --dry-run --json
openproject update 123 --status "In progress" --percent 40 --dry-run --json
openproject comment 123 --message "Implemented the API change."
openproject log-time 123 --hours 1.5 --date 2026-09-03 --comment "Implementation"
openproject commit-link HEAD --format url
openproject upgrade --dry-run --json
openproject uninstall --dry-run --json
```

## Operational rules

- Treat `create`, `update`, `comment`, and `log-time` as external writes; perform them only when the user explicitly requests that action.
- Treat `upgrade` as a local executable replacement; run it only when the user explicitly requests an upgrade.
- Treat `uninstall` as a destructive local action; run it only when the user explicitly requests removal of the executable.
- Fetch a work package immediately before an update so its `lockVersion` is current.
- Send relationship values through `_links` with `href`.
- Do not expose authorization headers, tokens, or secrets in output.
- If a comment or description includes a Git commit, use `openproject commit-link` to generate a clickable link when the remote can be safely resolved.
