#!/usr/bin/env sh
set -eu

REPOSITORY="${OPENPROJECT_RELEASE_REPOSITORY:-yungts97/openproject-skill}"
VERSION="${1:-latest}"
DESTINATION="${OPENPROJECT_INSTALL_DIR:-$HOME/.local/bin}"
case "$(uname -s)" in Darwin) OS="apple-darwin" ;; Linux) OS="unknown-linux-musl" ;; *) echo "Unsupported operating system. Use scripts/install.ps1 on Windows." >&2; exit 1 ;; esac
case "$(uname -m)" in x86_64|amd64) ARCH="x86_64" ;; arm64|aarch64) ARCH="aarch64" ;; *) echo "Unsupported processor architecture." >&2; exit 1 ;; esac
TARGET="${ARCH}-${OS}"; ARCHIVE="openproject-${TARGET}.tar.gz"; CHECKSUMS="SHA256SUMS"; TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT
if [ -n "${OPENPROJECT_GITLAB_PROJECT:-}" ]; then
  command -v glab >/dev/null || { echo "glab is required for OPENPROJECT_GITLAB_PROJECT." >&2; exit 1; }
  [ "$VERSION" != "latest" ] || { echo "Supply an explicit release tag when using OPENPROJECT_GITLAB_PROJECT." >&2; exit 1; }
  if [ -n "${OPENPROJECT_GITLAB_HOST:-}" ]; then
    glab release download "$VERSION" --hostname "$OPENPROJECT_GITLAB_HOST" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$CHECKSUMS" --dir "$TEMP_DIR"
  else
    glab release download "$VERSION" --repo "$OPENPROJECT_GITLAB_PROJECT" --pattern "$ARCHIVE" --pattern "$CHECKSUMS" --dir "$TEMP_DIR"
  fi
else
  BASE="https://github.com/${REPOSITORY}/releases"; if [ "$VERSION" = "latest" ]; then BASE="$BASE/latest/download"; else BASE="$BASE/download/v$VERSION"; fi
  curl --fail --location --silent --show-error "$BASE/$ARCHIVE" --output "$TEMP_DIR/$ARCHIVE"
  curl --fail --location --silent --show-error "$BASE/$CHECKSUMS" --output "$TEMP_DIR/$CHECKSUMS"
fi
CHECK_LINE="$(grep " $ARCHIVE$" "$TEMP_DIR/$CHECKSUMS")"
[ -n "$CHECK_LINE" ] || { echo "No checksum found for $ARCHIVE." >&2; exit 1; }
if command -v sha256sum >/dev/null; then printf '%s\n' "$CHECK_LINE" | (cd "$TEMP_DIR" && sha256sum --check -); else printf '%s\n' "$CHECK_LINE" | (cd "$TEMP_DIR" && shasum -a 256 -c -); fi
mkdir -p "$DESTINATION"; tar -xzf "$TEMP_DIR/$ARCHIVE" -C "$DESTINATION" openproject; chmod +x "$DESTINATION/openproject"
echo "Installed $DESTINATION/openproject"
