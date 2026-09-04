# Install OpenProject with an agent

These instructions are for the coding agent performing the installation. Install both the OpenProject Agent Skill and its CLI from `https://github.com/yungts97/openproject-skill`. They are separate installations: the skill supplies agent instructions, while the CLI is a platform-specific executable.

## Requirements

1. Run the repository installer for the current operating system: `scripts/install.sh` on Linux or macOS, or `scripts/install.ps1` on Windows. Use the latest release unless the user requested a version. The installer downloads and verifies both the platform-specific CLI and the release-pinned Agent Skill.
2. The default skill entrypoint is `~/.agents/skills/openproject/SKILL.md`, which is shared by Codex, OpenCode, and Pi. The installer also writes `~/.claude/skills/openproject/SKILL.md` when Claude Code is detected. Set `OPENPROJECT_SKILL_DIR` (or `-SkillDestination` in PowerShell) when the current agent requires a different user-level skill directory; do not install it in the current project.
3. Read the installed `SKILL.md`. Build the CLI from source only when no compatible release artifact is available.
4. Respect the environment's permission and approval requirements. Stop if a download, permission request, platform check, or checksum verification fails; do not bypass the failed safeguard.
5. Verify the CLI with `openproject --version` and `openproject --help`, and confirm its resolved executable path. If its directory is not on `PATH`, report the exact change required rather than silently editing shell startup files.
6. Confirm that the agent can discover the installed skill. If discovery requires restarting the agent or opening a new session, tell the user explicitly.
7. Do not ask the user to paste an OpenProject API token into chat, pass a token as a command argument, or save one in plaintext. If the installer offers to configure OpenProject interactively, decline it. Tell the user to run `openproject auth login` in their own interactive terminal after installation.
8. Finish by reporting the installed skill path, CLI path and version, verification results, and every remaining manual action.
