# Install OpenProject with an agent

These instructions are for the coding agent performing the installation. Install both the OpenProject Agent Skill and its CLI from `https://github.com/yungts97/openproject-skill`. They are separate installations: the skill supplies agent instructions, while the CLI is a platform-specific executable.

## Requirements

1. Determine the user-level skill directory supported by the current agent. Prefer its built-in skill installer when available. Otherwise, clone or download the repository into that directory as `openproject`; do not guess a directory or install it in the current project.
2. Ensure the skill entrypoint is `<skill-directory>/openproject/SKILL.md`. Avoid an extra nested repository directory. If an installation already exists, use its original installation mechanism to update it. Never overwrite uncommitted or user-authored changes; stop and report them instead.
3. Read the installed `SKILL.md`, then run the repository installer for the current operating system: `scripts/install.sh` on Linux or macOS, or `scripts/install.ps1` on Windows. Use the latest release unless the user requested a version. Build from source only when no compatible release artifact is available.
4. Respect the environment's permission and approval requirements. Stop if a download, permission request, platform check, or checksum verification fails; do not bypass the failed safeguard.
5. Verify the CLI with `openproject --version` and `openproject --help`, and confirm its resolved executable path. If its directory is not on `PATH`, report the exact change required rather than silently editing shell startup files.
6. Confirm that the agent can discover the installed skill. If discovery requires restarting the agent or opening a new session, tell the user explicitly.
7. Do not ask the user to paste an OpenProject API token into chat, pass a token as a command argument, or save one in plaintext. If the installer offers to configure OpenProject interactively, decline it. Tell the user to run `openproject auth login` in their own interactive terminal after installation.
8. Finish by reporting the installed skill path, CLI path and version, verification results, and every remaining manual action.
