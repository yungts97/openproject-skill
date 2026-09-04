#!/usr/bin/env sh
set -eu

usage() {
  cat <<'EOF'
Install or upgrade the OpenProject CLI.

Usage:
  install.sh [VERSION]

Arguments:
  VERSION  Release version to install (for example, 0.1.2 or v0.1.2).
           Defaults to the latest GitHub release.

Environment variables:
  OPENPROJECT_INSTALL_DIR         Installation directory (default: ~/.local/bin)
  OPENPROJECT_RELEASE_REPOSITORY  GitHub repository (default: yungts97/openproject-skill)
  OPENPROJECT_GITLAB_PROJECT      Private GitLab project used instead of GitHub
  OPENPROJECT_GITLAB_HOST         Optional hostname for a private GitLab instance
EOF
}

info() {
  printf '%s\n' "$1"
}

step() {
  printf '[%s/4] %s\n' "$1" "$2"
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
CHECKSUMS="SHA256SUMS"
EXECUTABLE="$DESTINATION/openproject"
STAGED=""
TEMP_DIR=""
ACTION="Installed"
[ ! -e "$EXECUTABLE" ] && [ ! -L "$EXECUTABLE" ] || ACTION="Upgraded"

cleanup() {
  if [ -n "$STAGED" ]; then
    rm -f "$STAGED"
  fi
  if [ -n "$TEMP_DIR" ]; then
    rm -rf "$TEMP_DIR"
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

info "OpenProject CLI installer"
info ""
info "  Version:     $VERSION"
info "  Target:      $TARGET"
info "  Destination: $EXECUTABLE"
info ""

step 1 "Checking system requirements"
require_command mktemp
require_command tar
require_command cp
require_command chmod
require_command mv

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

TEMP_DIR="$(mktemp -d)" || fail "Could not create a temporary directory."

step 2 "Downloading $ARCHIVE"
if [ -n "${OPENPROJECT_GITLAB_PROJECT:-}" ]; then
  if [ -n "${OPENPROJECT_GITLAB_HOST:-}" ]; then
    glab release download "$REQUESTED_VERSION" --hostname "$OPENPROJECT_GITLAB_HOST" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
  else
    glab release download "$REQUESTED_VERSION" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$CHECKSUMS" --dir "$TEMP_DIR" || fail "Could not download release $REQUESTED_VERSION from GitLab. Check the version and your glab authentication."
  fi
else
  BASE="https://github.com/${REPOSITORY}/releases"
  if [ "$VERSION" = "latest" ]; then
    BASE="$BASE/latest/download"
  else
    BASE="$BASE/download/v$VERSION"
  fi
  curl --fail --location --silent --show-error "$BASE/$ARCHIVE" --output "$TEMP_DIR/$ARCHIVE" || fail "Could not download $ARCHIVE. Check the release version and your network connection."
  curl --fail --location --silent --show-error "$BASE/$CHECKSUMS" --output "$TEMP_DIR/$CHECKSUMS" || fail "Could not download $CHECKSUMS. The release may be incomplete."
fi

step 3 "Verifying the SHA-256 checksum"
CHECK_LINE="$(
  while IFS= read -r line; do
    case "$line" in
      *" $ARCHIVE"|*" *$ARCHIVE")
        printf '%s\n' "$line"
        break
        ;;
    esac
  done < "$TEMP_DIR/$CHECKSUMS"
)"
[ -n "$CHECK_LINE" ] || fail "No checksum was published for $ARCHIVE."

if [ "$CHECKSUM_COMMAND" = "sha256sum" ]; then
  printf '%s\n' "$CHECK_LINE" | (cd "$TEMP_DIR" && sha256sum --check - >/dev/null) || fail "Checksum verification failed. The downloaded archive may be damaged or unsafe."
else
  printf '%s\n' "$CHECK_LINE" | (cd "$TEMP_DIR" && shasum -a 256 -c - >/dev/null) || fail "Checksum verification failed. The downloaded archive may be damaged or unsafe."
fi

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

info ""
info "Success: $ACTION $EXECUTABLE"
case ":${PATH:-}:" in
  *":$DESTINATION:"*) ;;
  *)
    info ""
    info "Note: $DESTINATION is not on PATH. Add this line to your shell profile:"
    printf '  export PATH="%s:$PATH"\n' "$DESTINATION"
    ;;
esac
info ""
info "Verify the installation:"
printf '  "%s" --version\n' "$EXECUTABLE"

if [ "$ACTION" = "Installed" ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    info ""
    printf 'Configure OpenProject now? [Y/n] '
    IFS= read -r CONFIGURE_NOW || CONFIGURE_NOW="n"
    case "$CONFIGURE_NOW" in
      n|N|[Nn][Oo])
        info "Run this later to configure securely:"
        printf '  "%s" auth login\n' "$EXECUTABLE"
        ;;
      *)
        if ! "$EXECUTABLE" auth login; then
          info ""
          info "OpenProject CLI was installed, but setup did not finish. Run this later:"
          printf '  "%s" auth login\n' "$EXECUTABLE"
        fi
        ;;
    esac
  else
    info ""
    info "Configure OpenProject later in an interactive terminal:"
    printf '  "%s" auth login\n' "$EXECUTABLE"
  fi
fi
