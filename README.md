# OpenProject CLI and Agent Skill

`openproject` is a portable OpenProject API v3 client. Its normal commands are non-interactive; `auth login` is the deliberate interactive setup command. This repository also contains an Agent Skill that teaches coding agents how to use the CLI safely for project and work-package operations.

The CLI supports Linux, macOS, and Windows on x86-64 and ARM64. It provides human-readable output, machine-readable JSON, dry runs for mutations, repository-aware project resolution, and configuration at global and project scope.

## Install with an agent

Paste this into Claude Code, OpenCode, Pi, Codex, or another agent that supports Agent Skills:

```text
Install the OpenProject Agent Skill and CLI by following https://raw.githubusercontent.com/yungts97/openproject-skill/main/INSTALL-WITH-AN-AGENT.md. Use its raw platform installer commands: they install the single `SKILL.md` file into the agent's skill directory and install the CLI.
```

## Manual installation

Run one command for your platform. It downloads the installer, matching CLI artifact, and Agent Skill from [GitHub Releases](https://github.com/yungts97/openproject-skill/releases), then verifies both release files against `SHA256SUMS` before installing them.

On Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/yungts97/openproject-skill/main/scripts/install.sh | sh
```

On Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/yungts97/openproject-skill/main/scripts/install.ps1 | iex
```

The default CLI destination is `~/.local/bin` on Linux and macOS, or `%LOCALAPPDATA%\openproject\bin` on Windows. The skill is installed to `~/.agents/skills/openproject/SKILL.md`, which is shared by Codex, OpenCode, and Pi. When Claude Code is detected, the installer also installs it to `~/.claude/skills/openproject/SKILL.md`. Set `OPENPROJECT_INSTALL_DIR` or `OPENPROJECT_SKILL_DIR` to override these destinations; on PowerShell, `-Destination` and `-SkillDestination` are also available. On a new interactive installation, the installer offers to launch secure OpenProject setup; non-interactive installations print the command to run later.

Ensure the destination directory is on `PATH`, then verify the installation:

```sh
openproject --version
openproject --help
```

To build from source instead, install a stable Rust toolchain and run:

```sh
cargo install --path .
```

## Upgrading

Upgrade to the latest GitHub release from the command line:

```sh
openproject upgrade
```

Pass a version without the leading `v` to install a specific release, or use `--dry-run` to inspect the source and destination without downloading anything:

```sh
openproject upgrade 0.2.0
openproject upgrade --dry-run --json
```

Rerunning the platform installation command also upgrades an existing executable in the destination directory. The installer verifies the downloaded archive, safely replaces the executable, and reports `Upgraded` instead of `Installed` when it finds an existing installation.

On Windows, `openproject upgrade` schedules the replacement immediately after the running process exits. The command targets the directory containing the executable, so it also works with a custom installation directory.

## Uninstallation

Remove the executable that is currently running:

```sh
openproject uninstall
```

Use `openproject uninstall --dry-run` to display the executable path without removing it. On Windows, removal is scheduled immediately after the command exits because a running executable cannot delete itself.

This command preserves global and repository configuration as well as the separately installed Agent Skill. Remove the skill through the agent or skill manager that installed it.

For a complete local cleanup, use `openproject uninstall --purge`. It removes the global configuration file, removes its directory only when empty, and deletes the stored credential for the configured host before removing the executable. If the global configuration is missing or invalid, pass `--host https://openproject.example.com` to identify the credential to remove. Repository `.openproject.json` files and the separately installed Agent Skill are always preserved. Use `--dry-run` to preview every target.

## Authentication

Create an API token in OpenProject under **My account → Access token**. For a persistent interactive setup, run:

```sh
openproject auth login
```

The guided setup asks for your server URL (showing `https://openproject.example.com` as an example), reads the token without echoing it, validates the credentials, stores the host in global configuration, and saves the token securely. It uses Keychain on macOS, Credential Manager on Windows, and Secret Service on Linux; an existing initialized `pass` store is used only when the native store is unavailable.

If no secure store is available—for example, on a headless Linux machine without `pass`—use `OPENPROJECT_TOKEN` for the current session or automation. The CLI never writes a plaintext token file.

Linux or macOS:

```sh
export OPENPROJECT_URL="https://openproject.example.com"
export OPENPROJECT_TOKEN="opapi-..."
openproject auth verify
```

Windows PowerShell:

```powershell
$env:OPENPROJECT_URL = "https://openproject.example.com"
$env:OPENPROJECT_TOKEN = "opapi-..."
openproject auth verify
```

`OPENPROJECT_TOKEN` overrides stored credentials, so it is suitable for CI and temporary sessions. Tokens are never accepted as command-line arguments or configuration-file values; do not commit them, include them in prompts, or place them in `.openproject.json`.

## Configuration

The CLI supports a global host setting and a repository-specific project mapping. Configuration files are optional, but a file that exists must contain valid JSON with supported fields and value types.

### Global scope

The global file accepts only `host`:

```json
{
  "host": "https://openproject.example.com"
}
```

Its platform-native location is:

| Platform | Path |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/openproject/config.json`, or `~/.config/openproject/config.json` when `XDG_CONFIG_HOME` is unset |
| macOS | `~/Library/Application Support/openproject/config.json` |
| Windows | `%APPDATA%\openproject\config.json` |

The global file intentionally cannot set a project. This prevents unrelated repositories from being routed to one default OpenProject project.

### Project scope

Place `.openproject.json` in the Git repository root. When `--cwd` is outside a Git repository, the file is read from that directory instead.

```json
{
  "host": "https://openproject.example.com",
  "project_id": 13
}
```

`project_id` may be a positive integer or non-empty string. The compatibility key `project` may contain a project name, identifier, or numeric ID. When both keys exist, `project_id` wins.

### Precedence

The host is resolved in this order:

1. `--host`
2. `OPENPROJECT_URL`
3. Project `.openproject.json`
4. Global `config.json`

The token is resolved in this order:

1. `OPENPROJECT_TOKEN`
2. The platform credential store
3. An initialized `pass` store

The project is resolved in this order:

1. A command's `--project`
2. Project `project_id` or `project`
3. An exact normalized match between the repository directory name and an OpenProject project name or identifier

If project resolution is missing or ambiguous, the CLI stops instead of guessing.

## Global options

Global options may be supplied before or after a subcommand.

| Option | Purpose |
| --- | --- |
| `-V`, `--version` | Print the CLI version and exit |
| `--host <URL>` | Override the configured OpenProject base URL |
| `--cwd <PATH>` | Choose the repository used for configuration and project discovery; defaults to `.` |
| `--json` | Emit JSON results; runtime errors are emitted as JSON on stderr |
| `--dry-run` | Preview a mutation without applying it |
| `-h`, `--help` | Show command help |

## Commands

| Command | Purpose and important arguments |
| --- | --- |
| `auth login` | Interactively validate and save the host and token in a secure credential store |
| `auth verify` | Validate the resolved URL and token by loading the current user |
| `projects` | List all visible OpenProject projects |
| `project [--project ID_OR_NAME]` | Resolve and display the project for the repository |
| `tasks [--project ID_OR_NAME] [--all] [--assignee ID_OR_ME] [--query TEXT]` | List project work packages; closed items are hidden unless `--all` is used |
| `task TASK_ID` | Show a work-package summary |
| `create --subject TEXT [OPTIONS]` | Create a work package; supports project, description, type/type ID, assignee, dates, and estimate |
| `update TASK_ID [OPTIONS]` | Update subject, description, status, assignee, percent complete, dates, or estimate |
| `comment TASK_ID --message TEXT` | Add an activity comment |
| `log-time TASK_ID --hours DURATION [OPTIONS]` | Log time with an optional date, comment, and activity ID |
| `commit-link COMMIT [--remote NAME] [--format html\|url\|json]` | Build a safe link for a GitHub, GitLab, Gitea, or Bitbucket commit |
| `upgrade [VERSION]` | Upgrade to the latest release, or to a specific version without the leading `v` |
| `uninstall` | Remove the running executable while preserving configuration and Agent Skill files |

Run `openproject COMMAND --help` for the full option list.

Dates use `YYYY-MM-DD`. Durations accept decimal hours that resolve to whole minutes, such as `1.5`, or ISO-8601 durations such as `PT1H30M`. `--assignee` accepts a numeric user ID or `me`. Statuses, types, and projects are matched exactly after case and punctuation normalization; numeric IDs avoid ambiguity.

Examples:

```sh
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

## Agent-friendly operation

All commands except `auth login` remain non-interactive, making them suitable for coding agents and automation.

- Use `--json` for deterministic structured results.
- Successful commands exit with code `0`. Runtime failures exit with code `1`; argument errors use Clap's non-zero exit behavior.
- With `--json`, runtime failures are written to stderr as `{"error":{"message":"..."}}`.
- Use `--dry-run --json` to inspect write requests before submitting them.
- `--version`, `--help`, `commit-link`, `upgrade`, and `uninstall` do not require OpenProject credentials.
- Treat `create`, `update`, `comment`, and `log-time` as external writes and run them only after the user authorizes the specific action.
- Resolve projects and named entities explicitly; never guess when multiple OpenProject values match.

## Private GitLab release mirrors

The public GitHub release is the default source. To use a private GitLab mirror, authenticate `glab`, set the project, and supply an explicit release version:

```sh
export OPENPROJECT_GITLAB_PROJECT="namespace/openproject-skill"
export OPENPROJECT_GITLAB_HOST="gitlab.example.com" # optional
./scripts/install.sh 0.1.0
```

The equivalent environment variables work with `install.ps1`. `OPENPROJECT_RELEASE_REPOSITORY` overrides the default public GitHub repository for either installer and for `openproject upgrade`. A GitLab-backed upgrade requires an explicit release version because `glab` does not resolve `latest` in this workflow.

## Development and releases

Run the Rust checks locally with:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```

Pushing a `v*` tag builds all supported platform archives and publishes them with `SHA256SUMS` through GitHub Actions.

## License

MIT
