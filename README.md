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

The default CLI destination is `~/.local/bin` on Linux and macOS, or `%LOCALAPPDATA%\openproject\bin` on Windows. The skill is installed to `~/.agents/skills/openproject/SKILL.md`, which is shared by Codex, OpenCode, and Pi. When Claude Code is detected, the installer also installs it to `~/.claude/skills/openproject/SKILL.md`. Set `OPENPROJECT_INSTALL_DIR` or `OPENPROJECT_SKILL_DIR` to override these destinations; on PowerShell, `-Destination` and `-SkillDestination` are also available.

When the default CLI directory is not persistently configured, the Unix installer adds one idempotent entry to the login shell's startup file and the PowerShell installer updates both the user PATH and its current process. Set `OPENPROJECT_NO_MODIFY_PATH=1` to opt out. Custom Unix destinations are left for you to add manually so the installer never writes an arbitrary value into a shell startup file.

On a new interactive installation, the installer offers to launch secure OpenProject setup. The Unix prompt also works with the documented `curl | sh` command by reading from the controlling terminal. Non-interactive installations print a prominent absolute `auth login` command instead; set `OPENPROJECT_NO_AUTH_PROMPT=1` to force this behavior when running through an agent or CI.

Open a new terminal if PATH was updated, then verify the installation:

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

`openproject upgrade` checks the latest release version before downloading its installer. If the running CLI is already current, it exits without downloading or replacing anything. Rerunning a platform installation command performs the same executable check; when the CLI is current, it skips the binary archive but downloads, verifies, and refreshes the matching Agent Skill. When an executable update is needed, the installer verifies the downloaded archive, safely replaces the executable, and reports `Upgraded` instead of `Installed` when it finds an existing installation.

On Windows, `openproject upgrade` schedules the replacement immediately after the running process exits. The command targets the directory containing the executable, so it also works with a custom installation directory.

## Uninstallation

Remove the executable that is currently running:

```sh
openproject uninstall
```

Use `openproject uninstall --dry-run` to display the executable path without removing it. On Windows, removal is scheduled immediately after the command exits because a running executable cannot delete itself.

This command preserves global and repository configuration as well as the separately installed Agent Skill. Remove the skill through the agent or skill manager that installed it.

For a complete local cleanup, use `openproject uninstall --purge`. It removes the global configuration file and the stored credential for the configured host before removing the executable. If that credential is the last entry in the protected credential file, the file is removed too; otherwise, credentials for other hosts remain. The command reports the precise credential stores removed and removes the configuration directory only when it is empty. If the global configuration is missing or invalid, pass `--host https://openproject.example.com` to identify the credential to remove. Repository `.openproject.json` files and the separately installed Agent Skill are always preserved. Use `--dry-run` to preview every target.

## Authentication

Create an API token in OpenProject under **My account → Access token**. For a persistent interactive setup, run:

```sh
openproject auth login
```

The guided setup reuses the host from project or global configuration when one is available, so it only asks for a server URL during first-time setup. Pass `--host URL` to use a different server. It validates the token before saving it and records the selected backend in global configuration. The CLI prefers the operating system credential manager where it is usable. In Linux, WSL, SSH, headless, and similar environments it uses a protected, user-only OpenProject credential file as the reliable fallback. `pass` is not used.

Authenticate once in the environment where the coding agent runs. Windows and WSL are separate environments, so run `openproject auth login` inside WSL when the agent runs there.

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

For containers, CI, Docker/Kubernetes secrets, and remote agents, `OPENPROJECT_TOKEN_FILE` is also supported; trailing whitespace is ignored:

```sh
OPENPROJECT_TOKEN_FILE=/run/secrets/openproject-token openproject task 1234
```

`OPENPROJECT_TOKEN` overrides `OPENPROJECT_TOKEN_FILE`, which overrides saved credentials. Tokens are never accepted as command-line arguments or repository configuration values; do not commit them, include them in prompts, or place them in `.openproject.json`.

### Saved login works in a terminal but fails in an agent

Run `openproject auth status --json` in the agent environment to diagnose setup without exposing a token. If no credential exists, run `openproject auth login` once in that environment.

## Configuration

The CLI supports a global host setting and a repository-specific project mapping. Configuration files are optional, but a file that exists must contain valid JSON with supported fields and value types.

### Global scope

The global file accepts `host` and the non-secret selected `credential_store`:

```json
{
  "host": "https://openproject.example.com",
  "credential_store": "file"
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
2. `OPENPROJECT_TOKEN_FILE`
3. The configured persistent credential backend
4. The safe file backend

The project is resolved in this order:

1. A command's `--project`
2. Project `project_id` or `project`
3. One unambiguous exact normalized match between an OpenProject project name or identifier and local project evidence: the Git-root directory name (or `--cwd` outside Git), the current directory name, the first README heading, or the `name` in `Cargo.toml`, `pyproject.toml`, `package.json`, or `composer.json`

The CLI treats conflicting evidence as ambiguous and stops instead of guessing. When no exact match exists, it reports up to five related projects based on shared meaningful words or a contained project name; these are suggestions only and must be selected explicitly with `--project` or saved in `.openproject.json`. It does not search arbitrary README text.

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
| `auth status` | Show credential availability and authentication status without exposing a token |
| `auth logout` | Remove the saved credential while retaining the configured host |
| `auth verify` | Validate the resolved URL and token by loading the current user |
| `projects [--limit N] [--offset N]` | List one page of visible OpenProject projects |
| `statuses [--limit N] [--offset N]` | List work package statuses |
| `priorities [--limit N] [--offset N]` | List work package priorities |
| `types [--project ID_OR_NAME]` | List work package types available in the resolved project |
| `users [--project ID_OR_NAME]` | List users who can be assigned work in the resolved project |
| `versions [--project ID_OR_NAME]` | List versions available in the resolved project |
| `categories [--project ID_OR_NAME]` | List work package categories available in the resolved project |
| `project [--project ID_OR_NAME] [--bind]` | Resolve and display the project; `--bind` explicitly saves its numeric ID to the repository configuration |
| `tasks [--project ID_OR_NAME] [FILTERS] [--sort FIELD:DIRECTION]` | List one server-filtered page of project work packages; supports assignee, subject, status, type, priority, due-date, recent-update, and repeated sort filters |
| `task TASK_ID [--full]` | Show a compact work-package summary, or its complete API representation with `--full` |
| `activities TASK_ID [--limit N] [--offset N]` | List a work package's activity/history entries with complete activity details |
| `activity ACTIVITY_ID` | Show one activity with its comment and change details |
| `time-entry-activities TASK_ID` | List the activity names and IDs allowed by the work package’s time-entry form |
| `relations TASK_ID [--limit N] [--offset N]` | List ordinary relations in which a work package is involved, plus its parent/child hierarchy links |
| `relation add FROM_ID --to TO_ID [OPTIONS]` | Create a typed relation, optionally with a description and lag |
| `relation delete RELATION_ID` | Delete a relation; supports the global `--dry-run` preview |
| `create --subject TEXT [OPTIONS]` | Create a work package; supports project, description, type, assignee, priority, responsible user, parent, version, dates, estimate, and custom fields |
| `update TASK_ID [OPTIONS]` | Update the create fields plus status and percent complete; deliberate `--clear-*` flags remove nullable values |
| `comment TASK_ID --message TEXT` | Add an activity comment |
| `log-time TASK_ID --hours DURATION [OPTIONS]` | Log time with an optional date, comment, and activity name or ID |
| `commit-link COMMIT [--remote NAME] [--format html\|url\|json]` | Build a safe link for a GitHub, GitLab, Gitea, or Bitbucket commit |
| `upgrade [VERSION]` | Upgrade to the latest release, or to a specific version without the leading `v` |
| `uninstall` | Remove the running executable while preserving configuration and Agent Skill files |

Run `openproject COMMAND --help` for the full option list.

Dates use `YYYY-MM-DD`. Durations accept decimal hours that resolve to whole minutes, such as `1.5`, or ISO-8601 durations such as `PT1H30M`. User fields accept a numeric user ID or `me`. Statuses, priorities, types, versions, and projects are matched exactly after case and punctuation normalization; numeric IDs avoid ambiguity.

Use repeated `--custom-field customFieldN=JSON` arguments for scalar custom fields. If the value is not valid JSON, it is treated as a string. Use `--custom-field-link customFieldN=/api/v3/RESOURCE/ID` for linked custom fields. The numeric property name and value type come from the OpenProject work package schema; the CLI deliberately does not guess them.

Examples:

```sh
openproject projects --json
openproject project --project 13 --json
openproject project --project 13 --bind --json
openproject statuses --json
openproject users --project 13 --json
openproject tasks --project 13 --assignee me --status "In progress" --priority High --due-before 2026-09-30 --updated-since 7d --sort priority:desc --sort updated-at:desc --json
openproject task 123 --full --json
openproject activities 123 --limit 50 --json
openproject activity 456 --json
openproject time-entry-activities 123 --json
openproject relations 123 --json
openproject relation add 123 --to 456 --type blocks --dry-run --json
openproject relation delete 789 --dry-run --json
openproject create --project 13 --subject "Fix approval flow" --type Task --assignee me --priority High --version "Release 2" --custom-field customField1=Acme --dry-run --json
openproject update 123 --status "In progress" --percent 40 --responsible me --clear-due-date --dry-run --json
openproject comment 123 --message "Implemented the API change."
openproject log-time 123 --hours 1.5 --date 2026-09-03 --comment "Implementation" --activity Development
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
- Collection commands return one page by default. Use `--limit` and `--offset`; task-list output includes `next`, `total`, and page metadata.
- `tasks` sends all filters and sorting to OpenProject instead of downloading and filtering work packages locally. Repeating `--status`, `--type`, or `--priority` creates an OR list within that field; different fields are combined with AND.
- `--updated-since` accepts a positive day count such as `7` or `7d`. `--sort` accepts a documented field with optional `asc` or `desc` and may be repeated for secondary sorting.
- `--clear-description`, `--clear-assignee`, `--clear-responsible`, `--clear-parent`, `--clear-version`, `--clear-start-date`, `--clear-due-date`, `--clear-estimate`, and the custom-field clear options intentionally send a null value. A clear option cannot be combined with its corresponding value option.
- `project --bind` is an explicit local write to `.openproject.json`; agents must still obtain the repository-binding approval described in the Agent Skill. Its `--dry-run` output previews the target file and resolved ID without writing.
- `--version`, `--help`, `commit-link`, `upgrade`, and `uninstall` do not require OpenProject credentials.
- Treat `create`, `update`, `comment`, `log-time`, and `relation add/delete` as external writes and run them only after the user authorizes the specific action.
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
