# Install OpenProject with an agent

These instructions are for the coding agent performing the installation. The OpenProject Agent Skill is the single `SKILL.md` file; the platform installer places it in the agent's user-level skill directory and installs the platform-specific CLI.

## Installation

1. Obtain approval if the environment requires it, then run the raw installer for the current operating system. For Linux or macOS:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/yungts97/openproject-skill/main/scripts/install.sh | sh
   ```

   For Windows PowerShell:

   ```powershell
   irm https://raw.githubusercontent.com/yungts97/openproject-skill/main/scripts/install.ps1 | iex
   ```

   For a requested Unix version, append it through `sh`, for example `curl -fsSL https://raw.githubusercontent.com/yungts97/openproject-skill/main/scripts/install.sh | sh -s -- 0.1.3`. On PowerShell, download the script to a temporary file and invoke it with `-Version`.

2. The installer downloads and verifies both the platform-specific CLI and the release-pinned `SKILL.md`. It writes the skill to `~/.agents/skills/openproject/SKILL.md`, which is shared by Codex, OpenCode, and Pi. When Claude Code is detected, it also writes `~/.claude/skills/openproject/SKILL.md`. Set `OPENPROJECT_SKILL_DIR` (or `-SkillDestination` in PowerShell) to select another user-level skill directory.
3. Read the installed `SKILL.md`. Build the CLI from source only when no compatible release artifact is available.
4. Respect the environment's permission and approval requirements. Stop if a download, permission request, platform check, or checksum verification fails; do not bypass the failed safeguard.
5. Verify the CLI with `openproject --version` and `openproject --help`, and confirm its resolved executable path. If its directory is not on `PATH`, report the exact change required rather than silently editing shell startup files.
6. Confirm that the agent can discover the installed skill. If discovery requires restarting the agent or opening a new session, tell the user explicitly.
7. Do not ask the user to paste an OpenProject API token into chat, pass a token as a command argument, or save one in plaintext. If the installer offers to configure OpenProject interactively, decline it. Tell the user to run `openproject auth login` in their own interactive terminal after installation.
8. Finish by reporting the installed skill path, CLI path and version, verification results, and every remaining manual action.
