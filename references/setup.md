# Setup and local lifecycle

Read when installation, authentication, upgrade, or removal is relevant. The public skill source is the repository root of `yungts97/openproject-skill`.

## Install

Check `openproject --version`. If the CLI or skill is missing, explain that the platform installer downloads the release-pinned CLI and skill files, verifying SHA-256 checksums. A request to install authorizes that installation; otherwise obtain approval before running `scripts/install.sh` on Linux/macOS or `scripts/install.ps1` on Windows. Stop on failed downloads, platform checks, permissions, or checksums.

The installer installs the CLI and skill together. `OPENPROJECT_SKILL_DIR` selects a nonstandard user-level skill directory. A private GitLab mirror can use `OPENPROJECT_GITLAB_PROJECT`, optionally `OPENPROJECT_GITLAB_HOST`, and an existing `glab` login.

The installer adds the default executable directory to PATH: one idempotent shell-startup entry on Unix, user PATH plus the current PowerShell process on Windows. `OPENPROJECT_NO_MODIFY_PATH=1` opts out. The running agent may retain its old PATH; use the installer's absolute executable path when needed and report any remaining manual PATH or agent-session restart action.

## Authenticate

Authentication persists. Run `openproject auth status --json` when authentication fails; if no credential exists, direct the user to `openproject auth login` once in this environment. The CLI uses a system credential manager where suitable and a protected-file fallback in headless, WSL, SSH, and container environments.

For an agent-owned/noninteractive installation, set `OPENPROJECT_NO_AUTH_PROMPT=1`, show the installer's absolute `auth login` command, and have the user run it in their own terminal. A user-run installer can offer login immediately, including the documented `curl | sh` flow. Never ask for a token in chat, print one, pass one as an argument, or save one in repository configuration. Verify `auth status --json` after the user completes login before reporting the setup ready.

For CI or temporary sessions without saved credentials, the user may supply `OPENPROJECT_TOKEN` through the process environment and run `openproject auth verify`; do not handle the secret on their behalf.

## Upgrade and removal

- `openproject upgrade [VERSION]` downloads and replaces the executable; versions omit the leading `v`. Rerunning the platform installer also upgrades an existing executable and refreshes the skill. Proceed when an upgrade was requested; otherwise obtain approval. Use `--dry-run --json` if the source or destination needs review.
- `openproject uninstall` removes the executable while preserving global/repository configuration and the separately installed skill. Remove the skill through the agent/skill manager that installed it. Use `--dry-run` if the executable path needs review.
- `openproject uninstall --purge` requires an explicit request for complete local cleanup. It removes global configuration and the configured host's credential; other hosts' credentials remain. It removes `credentials.json` only when the final protected-file credential is removed. Repository `.openproject.json` and the separately installed skill remain. If global config is missing/invalid, use `--host` to identify the credential.
