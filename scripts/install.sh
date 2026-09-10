#!/usr/bin/env sh
set -eu

usage() {
  cat <<'EOF'
Install or upgrade the OpenProject CLI and Agent Skill.

Usage:
  install.sh [VERSION]

Arguments:
  VERSION  Release version to install (for example, 0.1.2 or v0.1.2).
           Defaults to the latest GitHub release.

Environment variables:
  OPENPROJECT_INSTALL_DIR         Installation directory (default: ~/.local/bin)
  OPENPROJECT_SKILL_DIR           Agent Skills directory (default: ~/.agents/skills)
  OPENPROJECT_RELEASE_REPOSITORY  GitHub repository (default: yungts97/openproject-skill)
  OPENPROJECT_GITLAB_PROJECT      Private GitLab project used instead of GitHub
  OPENPROJECT_GITLAB_HOST         Optional hostname for a private GitLab instance
  OPENPROJECT_NO_MODIFY_PATH      Set to 1 to leave shell startup files unchanged
  OPENPROJECT_NO_AUTH_PROMPT      Set to 1 to skip interactive authentication setup
EOF
}

info() {
  printf '%s\n' "$1"
}

step() {
  printf '[%s/5] %s\n' "$1" "$2"
}

fail() {
  printf '\nError: %s\n' "$1" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "Required command '$1' was not found on PATH."
}

REPOSITORY="${OPENPROJECT_RELEASE_REPOSITORY:-yungts97/openproject-skill}"
REQUESTED_VERSION="latest"
VERSION_SUPPLIED=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    -* )
      usage >&2
      fail "Unsupported option '$1'."
      ;;
    *)
      [ "$VERSION_SUPPLIED" -eq 0 ] || {
        usage >&2
        fail "Expected at most one version argument."
      }
      REQUESTED_VERSION="$1"
      VERSION_SUPPLIED=1
      ;;
  esac
  shift
done
VERSION="${REQUESTED_VERSION#v}"
[ -n "$VERSION" ] || fail "The release version cannot be empty."

if [ -n "${OPENPROJECT_INSTALL_DIR:-}" ]; then
  DESTINATION="$OPENPROJECT_INSTALL_DIR"
else
  [ -n "${HOME:-}" ] || fail "HOME is not set. Set OPENPROJECT_INSTALL_DIR to choose an installation directory."
  DESTINATION="$HOME/.local/bin"
fi

if [ -n "${OPENPROJECT_SKILL_DIR:-}" ]; then
  SKILL_ROOT="$OPENPROJECT_SKILL_DIR"
else
  [ -n "${HOME:-}" ] || fail "HOME is not set. Set OPENPROJECT_SKILL_DIR to choose an Agent Skills directory."
  SKILL_ROOT="$HOME/.agents/skills"
fi

CLAUDE_SKILL_ROOT=""
if [ -z "${OPENPROJECT_SKILL_DIR:-}" ] && { command -v claude >/dev/null 2>&1 || [ -d "$HOME/.claude" ]; }; then
  CLAUDE_SKILL_ROOT="$HOME/.claude/skills"
fi

case "$(uname -s)" in
  Darwin) OS="apple-darwin" ;;
  Linux) OS="unknown-linux-musl" ;;
  *) fail "This operating system is not supported by install.sh. On Windows, use scripts/install.ps1." ;;
esac

case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *) fail "Processor architecture '$(uname -m)' is not supported. Supported architectures: x86_64 and arm64." ;;
esac

TARGET="${ARCH}-${OS}"
ARCHIVE="openproject-${TARGET}.tar.gz"
SKILL_ASSET="openproject-agent-skill.md"
CHECKSUMS="SHA256SUMS"
EXECUTABLE="$DESTINATION/openproject"
STAGED=""
TEMP_DIR=""
ACTION="Installed"
[ ! -e "$EXECUTABLE" ] && [ ! -L "$EXECUTABLE" ] || ACTION="Upgraded"
CLI_CURRENT=0

cleanup() {
  if [ -n "$STAGED" ]; then
    rm -f "$STAGED"
  fi
  if [ -n "$TEMP_DIR" ]; then
    rm -rf "$TEMP_DIR"
  fi
}

verify_checksum() {
  ASSET="$1"
  CHECK_LINE="$(awk -v asset="$ASSET" 'length($1) == 64 && $1 ~ /^[[:xdigit:]]+$/ && ($2 == asset || $2 == "*" asset) { print; exit }' "$TEMP_DIR/$CHECKSUMS")"
  [ -n "$CHECK_LINE" ] || fail "No checksum was published for $ASSET."
  EXPECTED_CHECKSUM="${CHECK_LINE%%[[:space:]]*}"

  if [ "$CHECKSUM_COMMAND" = "sha256sum" ]; then
    ACTUAL_CHECKSUM="$(sha256sum "$TEMP_DIR/$ASSET")" || fail "Could not calculate the SHA-256 checksum for $ASSET."
  else
    ACTUAL_CHECKSUM="$(shasum -a 256 "$TEMP_DIR/$ASSET")" || fail "Could not calculate the SHA-256 checksum for $ASSET."
  fi
  ACTUAL_CHECKSUM="${ACTUAL_CHECKSUM%%[[:space:]]*}"

  [ "$ACTUAL_CHECKSUM" = "$EXPECTED_CHECKSUM" ] || fail "Checksum verification failed for $ASSET. The downloaded file may be damaged or unsafe."
}

installed_version() {
  [ -x "$EXECUTABLE" ] || return 1
  INSTALLED_VERSION="$("$EXECUTABLE" --version 2>/dev/null)" || return 1
  INSTALLED_VERSION="${INSTALLED_VERSION#openproject }"
  printf '%s\n' "${INSTALLED_VERSION%% *}"
}

install_skill() {
  ROOT="$1"
  SKILL_DIRECTORY="$ROOT/openproject"
  SKILL_FILE="$SKILL_DIRECTORY/SKILL.md"
  SKILL_ACTION="Installed"
  [ ! -e "$SKILL_FILE" ] && [ ! -L "$SKILL_FILE" ] || SKILL_ACTION="Upgraded"

  mkdir -p "$SKILL_DIRECTORY" || fail "Could not create $SKILL_DIRECTORY. Set OPENPROJECT_SKILL_DIR to a writable Agent Skills directory."
  [ -w "$SKILL_DIRECTORY" ] || fail "$SKILL_DIRECTORY is not writable. Set OPENPROJECT_SKILL_DIR to a writable Agent Skills directory."
  STAGED="$SKILL_DIRECTORY/.SKILL.md.new.$$"
  cp "$TEMP_DIR/$SKILL_ASSET" "$STAGED" || fail "Could not stage the OpenProject Agent Skill in $SKILL_DIRECTORY."
  mv -f "$STAGED" "$SKILL_FILE" || fail "Could not replace $SKILL_FILE."
  STAGED=""
  info "  $SKILL_ACTION $SKILL_FILE"
}

print_manual_path_setup() {
  case "${SHELL:-}" in
    */fish)
      info "Note: $DESTINATION is not on PATH. Add it with fish:"
      printf '  fish_add_path "%s"\n' "$DESTINATION"
      ;;
    *)
      info "Note: $DESTINATION is not on PATH. Add this line to your shell profile:"
      printf '  export PATH="%s:$PATH"\n' "$DESTINATION"
      ;;
  esac
}

configure_path() {
  PATH_IS_ACTIVE=0
  case ":${PATH:-}:" in
    *":$DESTINATION:"*) PATH_IS_ACTIVE=1 ;;
  esac

  if [ "${OPENPROJECT_NO_MODIFY_PATH:-}" = "1" ]; then
    if [ "$PATH_IS_ACTIVE" -eq 0 ]; then
      info ""
      print_manual_path_setup
    fi
    return
  fi

  if [ -z "${HOME:-}" ] || [ "$DESTINATION" != "$HOME/.local/bin" ]; then
    if [ "$PATH_IS_ACTIVE" -eq 0 ]; then
      info ""
      print_manual_path_setup
    fi
    return
  fi

  case "${SHELL:-}" in
    */bash)
      if [ "$OS" = "apple-darwin" ]; then
        PROFILE="$HOME/.bash_profile"
      else
        PROFILE="$HOME/.bashrc"
      fi
      PATH_LINE='export PATH="$HOME/.local/bin:$PATH"'
      ;;
    */zsh)
      PROFILE="$HOME/.zshrc"
      PATH_LINE='export PATH="$HOME/.local/bin:$PATH"'
      ;;
    */fish)
      PROFILE="$HOME/.config/fish/config.fish"
      PATH_LINE='fish_add_path "$HOME/.local/bin"'
      mkdir -p "$HOME/.config/fish" || {
        print_manual_path_setup
        return
      }
      ;;
    */sh|*/dash|*/ksh)
      PROFILE="$HOME/.profile"
      PATH_LINE='export PATH="$HOME/.local/bin:$PATH"'
      ;;
    *)
      if [ "$PATH_IS_ACTIVE" -eq 0 ]; then
        info ""
        info "Automatic PATH setup is not available for ${SHELL:-this shell}."
        print_manual_path_setup
      fi
      return
      ;;
  esac

  if ! grep -Fqx "$PATH_LINE" "$PROFILE" 2>/dev/null; then
    info ""
    if ! printf '\n# Added by the OpenProject CLI installer\n%s\n' "$PATH_LINE" >> "$PROFILE"; then
      info "Could not update $PROFILE automatically."
      if [ "$PATH_IS_ACTIVE" -eq 0 ]; then
        print_manual_path_setup
      fi
      return
    fi
    info "Added $DESTINATION to PATH in $PROFILE."
  else
    if [ "$PATH_IS_ACTIVE" -eq 1 ]; then
      return
    fi
    info ""
    info "$DESTINATION is already configured in $PROFILE."
  fi
  if [ "$PATH_IS_ACTIVE" -eq 0 ]; then
    PATH="$DESTINATION${PATH:+:$PATH}"
    export PATH
  fi
  info "It will be available automatically in new terminal sessions."
}

print_auth_handoff() {
  info ""
  info "ACTION REQUIRED: Finish OpenProject setup in an interactive terminal:"
  printf '  "%s" auth login\n' "$EXECUTABLE"
}

configure_auth() {
  if [ "${OPENPROJECT_NO_AUTH_PROMPT:-}" = "1" ]; then
    print_auth_handoff
    return
  fi

  PROMPT_INPUT=""
  if [ -t 1 ]; then
    if [ -t 0 ]; then
      PROMPT_INPUT="stdin"
    elif ( : </dev/tty ) 2>/dev/null; then
      PROMPT_INPUT="tty"
    fi
  fi

  if [ -z "$PROMPT_INPUT" ]; then
    print_auth_handoff
    return
  fi

  info ""
  printf 'Configure OpenProject now? [Y/n] '
  if [ "$PROMPT_INPUT" = "tty" ]; then
    IFS= read -r CONFIGURE_NOW </dev/tty || CONFIGURE_NOW="n"
  else
    IFS= read -r CONFIGURE_NOW || CONFIGURE_NOW="n"
  fi

  case "$CONFIGURE_NOW" in
    n|N|[Nn][Oo]) print_auth_handoff ;;
    *)
      if [ "$PROMPT_INPUT" = "tty" ]; then
        "$EXECUTABLE" auth login </dev/tty || print_auth_handoff
      else
        "$EXECUTABLE" auth login || print_auth_handoff
      fi
      ;;
  esac
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

step 1 "Checking system requirements"
require_command mktemp
require_command tar
require_command cp
require_command chmod
require_command mv
require_command awk
require_command grep

if [ -n "${OPENPROJECT_GITLAB_PROJECT:-}" ]; then
  [ "$VERSION" != "latest" ] || fail "A specific release version is required with OPENPROJECT_GITLAB_PROJECT."
  require_command glab
else
  require_command curl
fi

if command -v sha256sum >/dev/null 2>&1; then
  CHECKSUM_COMMAND="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  CHECKSUM_COMMAND="shasum"
else
  fail "A SHA-256 tool is required. Install 'sha256sum' or 'shasum' and try again."
fi

if [ "$VERSION" = "latest" ] && [ -z "${OPENPROJECT_GITLAB_PROJECT:-}" ]; then
  LATEST_RELEASE_URL="$(curl --fail --head --location --silent --show-error \
    --output /dev/null --write-out '%{url_effective}' \
    "https://github.com/${REPOSITORY}/releases/latest")" \
    || fail "Could not check the latest release. Check your network connection."
  VERSION="${LATEST_RELEASE_URL##*/}"
  VERSION="${VERSION#v}"
  [ -n "$VERSION" ] || fail "Could not determine the latest release version."
fi

if [ "$(installed_version || true)" = "$VERSION" ]; then
  info "OpenProject $VERSION is already installed; no upgrade needed."
  CLI_CURRENT=1
fi

info "OpenProject CLI and Agent Skill installer"
info ""
info "  Version:     $VERSION"
info "  Target:      $TARGET"
info "  Destination: $EXECUTABLE"
info "  Agent Skill: $SKILL_ROOT/openproject/SKILL.md"
info ""

TEMP_DIR="$(mktemp -d)" || fail "Could not create a temporary directory."

step 2 "Downloading release assets"
if [ -n "${OPENPROJECT_GITLAB_PROJECT:-}" ]; then
  if [ -n "${OPENPROJECT_GITLAB_HOST:-}" ]; then
    if [ "$CLI_CURRENT" -eq 1 ]; then
      glab release download "$REQUESTED_VERSION" --hostname "$OPENPROJECT_GITLAB_HOST" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$SKILL_ASSET" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
    else
      glab release download "$REQUESTED_VERSION" --hostname "$OPENPROJECT_GITLAB_HOST" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$SKILL_ASSET" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
    fi
  else
    if [ "$CLI_CURRENT" -eq 1 ]; then
      glab release download "$REQUESTED_VERSION" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$SKILL_ASSET" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
    else
      glab release download "$REQUESTED_VERSION" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$SKILL_ASSET" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
    fi
  fi
else
  BASE="https://github.com/${REPOSITORY}/releases"
  if [ "$VERSION" = "latest" ]; then
    BASE="$BASE/latest/download"
  else
    BASE="$BASE/download/v$VERSION"
  fi
  if [ "$CLI_CURRENT" -eq 0 ]; then
    curl --fail --location --silent --show-error "$BASE/$ARCHIVE" --output "$TEMP_DIR/$ARCHIVE" || fail "Could not download $ARCHIVE. Check the release version and your network connection."
  fi
  curl --fail --location --silent --show-error "$BASE/$SKILL_ASSET" --output "$TEMP_DIR/$SKILL_ASSET" || fail "Could not download $SKILL_ASSET. The release may be incomplete."
  curl --fail --location --silent --show-error "$BASE/$CHECKSUMS" --output "$TEMP_DIR/$CHECKSUMS" || fail "Could not download $CHECKSUMS. The release may be incomplete."
fi

step 3 "Verifying SHA-256 checksums"
[ "$CLI_CURRENT" -eq 1 ] || verify_checksum "$ARCHIVE"
verify_checksum "$SKILL_ASSET"

if [ "$CLI_CURRENT" -eq 1 ]; then
  step 4 "Keeping the current OpenProject CLI"
else
  step 4 "$ACTION OpenProject CLI"
  mkdir -p "$DESTINATION" || fail "Could not create $DESTINATION. Set OPENPROJECT_INSTALL_DIR to a writable directory."
  [ -w "$DESTINATION" ] || fail "$DESTINATION is not writable. Set OPENPROJECT_INSTALL_DIR to a writable directory."
  tar -xzf "$TEMP_DIR/$ARCHIVE" -C "$TEMP_DIR" openproject || fail "Could not extract the OpenProject executable from $ARCHIVE."
  [ -f "$TEMP_DIR/openproject" ] || fail "The release archive does not contain the OpenProject executable."

  STAGED="$DESTINATION/.openproject.new.$$"
  cp "$TEMP_DIR/openproject" "$STAGED" || fail "Could not stage the OpenProject executable in $DESTINATION."
  chmod +x "$STAGED" || fail "Could not make the OpenProject executable runnable."
  mv -f "$STAGED" "$EXECUTABLE" || fail "Could not replace $EXECUTABLE. Make sure it is not in use and try again."
  STAGED=""
fi

step 5 "Installing OpenProject Agent Skill"
install_skill "$SKILL_ROOT"
if [ -n "$CLAUDE_SKILL_ROOT" ] && [ "$CLAUDE_SKILL_ROOT" != "$SKILL_ROOT" ]; then
  install_skill "$CLAUDE_SKILL_ROOT"
fi

info ""
if [ "$CLI_CURRENT" -eq 1 ]; then
  info "Success: OpenProject $VERSION is current and the Agent Skill was refreshed"
else
  info "Success: $ACTION $EXECUTABLE and installed the OpenProject Agent Skill"
fi
configure_path
info ""
info "Verify the installation:"
printf '  "%s" --version\n' "$EXECUTABLE"
info "Restart your agent session if it does not detect the newly installed skill."

if [ "$ACTION" = "Installed" ]; then
  configure_auth
else
  info ""
  info "Check authentication in this environment:"
  printf '  "%s" auth status --json\n' "$EXECUTABLE"
  info "If it is not authenticated, run:"
  printf '  "%s" auth login\n' "$EXECUTABLE"
fi
