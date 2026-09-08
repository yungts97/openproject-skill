---
name: openproject
description: Manage OpenProject projects and work packages through API v3. Use when a user asks to inspect, create, update, comment on, or log time against OpenProject work packages.
metadata:
  short-description: Manage OpenProject work packages
---

# OpenProject

Use the bundled `openproject` CLI. It is portable and non-interactive by default; only `openproject auth login` prompts deliberately for local credential setup.

## Installation and setup

Check availability with `openproject --version`. If the executable or this Agent Skill is missing, explain that the platform installer downloads the release-pinned CLI and skill and verifies their SHA-256 checksums, then obtain approval before running `scripts/install.sh` on Linux/macOS or `scripts/install.ps1` on Windows.

Upgrade an existing executable with `openproject upgrade`, optionally followed by a version without the leading `v`. Use `openproject upgrade --dry-run --json` when the source or destination needs review. Rerunning the platform installer also detects and upgrades an existing executable. Obtain approval before either upgrade path because it downloads and replaces the local executable.

The public skill source is the repository root of `yungts97/openproject-skill`. The platform installer installs the CLI and skill together; set `OPENPROJECT_SKILL_DIR` when the agent requires a nonstandard user-level skill directory. Users with a private GitLab mirror may set `OPENPROJECT_GITLAB_PROJECT`, optionally `OPENPROJECT_GITLAB_HOST`, and use their existing `glab` login.

Remove the executable with `openproject uninstall`. Use `--dry-run` first when the resolved executable path needs review. This preserves configuration and the separately installed Agent Skill; remove the skill through the agent or skill manager that installed it. Use `openproject uninstall --purge` only when the user explicitly requests complete local cleanup: it removes global configuration and the stored credential for the configured host. When that is the final protected-file credential, it removes `credentials.json` as well; credentials for other hosts remain. Repository `.openproject.json` files and the separately installed Agent Skill are always preserved. If global configuration is missing or invalid, pass `--host` to identify the credential to remove.

Authentication is persistent. Never ask the user to paste a token into chat, print it, place it in command arguments, or write it to repository configuration. If authentication fails, run `openproject auth status --json`. If no credential exists, tell the user: `Run openproject auth login once in this environment.` The CLI uses a system credential manager where suitable and a protected credential file fallback for agent, WSL, SSH, headless, and container-friendly environments.

For CI, headless machines, and temporary sessions without accessible saved credentials, the user can supply a token through the process environment:

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

### Current-directory project detection

At the start of work that needs a project, find the Git root (or use the current directory when outside a Git repository) and inspect its `.openproject.json`. If it contains a valid `project_id`, treat that as the repository's project binding and use it by default for all later project-scoped OpenProject queries relevant to this repository. Run `openproject project --cwd . --json` (or pass the known repository root as `--cwd`) to resolve and verify the configured project.

**Mandatory decision gate:** If the repository configuration has no valid `project_id`, do not run the user's project-scoped command yet—even for a read such as `openproject tasks`. First resolve a project using the process below, then obtain the user's project-selection and persistence decision. Authentication, directory-name matching, and a CLI-resolved default never authorize silently using or persisting a repository project binding. `openproject project --cwd . --json` and `openproject projects --json` are permitted only to perform this resolution.

If `.openproject.json` is missing or does not contain `project_id`, let the CLI use local project evidence to attempt an exact normalized match against an OpenProject project name or identifier. Evidence includes the project directory name, the current directory name, the first README heading, and a `name` from common manifests (`Cargo.toml`, `pyproject.toml`, `package.json`, or `composer.json`). If necessary, inspect `openproject projects --json`. Accept an exact result only when all matching evidence identifies one project. When no exact match exists, use the CLI's related-project suggestions (shared meaningful words or a contained project name) only as candidates to show the user; never select a related project, search arbitrary README text, or make a speculative match.

When an exact project is found, stop before running the requested project-scoped command. Tell the user its name and ID and present explicit choices for the repository-root `.openproject.json` decision; do not ask an open-ended question. Offer: **Yes — create/update with this project ID**, **No — use it only for this request**, and **Use a different project ID — let the user type an ID**. Do not create or modify the file without the user's permission. If the current request already explicitly authorizes this configuration change, do not ask again. Once authorized, create the file when absent or update it when present, preserving `host` and other supported settings, then use the persisted `project_id` for subsequent relevant queries. If they choose use-once, run the requested command with an explicit `--project <resolved-id>` and do not write the file. Treat this local configuration change separately from OpenProject API writes.

If the match is missing or ambiguous, show the viable candidates and present explicit choices: one choice per candidate, **Use a different project ID — let the user type an ID**, and **Do not select a project**. After a project is selected or typed, present the same explicit persistence choices: **Yes — create/update `.openproject.json` with this ID** or **No — use it only for this request**. If they decline persistence, do not write the file; use an explicit `--project` only for the current request.

Read repository guidance before external writes. Use an explicit `--project` when guidance supplies one; it overrides the directory default for that command.

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
openproject uninstall --purge --dry-run --json
```

## Operational rules

- Treat `create`, `update`, `comment`, and `log-time` as external writes; perform them only when the user explicitly requests that action.
- Treat `upgrade` as a local executable replacement; run it only when the user explicitly requests an upgrade.
- Treat `uninstall` as a destructive local action; run it only when the user explicitly requests removal of the executable. `--purge` additionally removes global configuration and securely stored credentials.
- Fetch a work package immediately before an update so its `lockVersion` is current.
- Send relationship values through `_links` with `href`.
- Do not expose authorization headers, tokens, or secrets in output.
- If a comment or description includes a Git commit, use `openproject commit-link` to generate a clickable link when the remote can be safely resolved.
