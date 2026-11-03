#!/usr/bin/env bash
#
# Installs vendor on Linux and macOS, with a global config in ~/vendor.yml.
#
#   curl -fsSL https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/install.sh | bash

# Everything runs from main, so a download cut short executes nothing.

set -euo pipefail

REPOSITORY="https://github.com/Araxeus/vendorfiles-rs"

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

release_os() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) echo linux ;;
        Darwin-arm64) echo macos ;;
        *) die "no release for $(uname -s) on $(uname -m). See $REPOSITORY#installation for other ways to install." ;;
    esac
}

require_tools() {
    local tool
    for tool in curl tar; do
        command -v "$tool" >/dev/null ||
            die "$tool is required. Install it with your package manager, then run this again."
    done
}

# The redirect of /releases/latest names the tag and uses no GitHub API quota.
latest_tag() {
    local url
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPOSITORY/releases/latest") ||
        die "could not reach $REPOSITORY"
    case "${url##*/}" in
        v*) echo "${url##*/}" ;;
        *) die "could not read the latest version from $url" ;;
    esac
}

# Prints vendorConfig.vendorFolder from the config, with ~ and relative paths resolved.
configured_folder() {
    local folder
    folder=$(awk '
        { sub(/\r$/, "") }
        /^vendorConfig:/ { inside = 1; next }
        /^[^ \t#]/ { inside = 0 }
        inside && /^[ \t]+vendorFolder:/ {
            sub(/^[ \t]+vendorFolder:[ \t]*/, "")
            sub(/[ \t]+#.*$/, "")
            gsub(/^["\047]|["\047]$/, "")
            print
            exit
        }
    ' "$1")

    case "$folder" in
        "") return 1 ;;
        \~) echo "$HOME" ;;
        \~/*) echo "$HOME/${folder#\~/}" ;;
        /*) echo "$folder" ;;
        *) echo "$(dirname "$1")/${folder#./}" ;;
    esac
}

default_folder() {
    if [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
        echo /usr/local/bin
    else
        echo "$HOME/.local/bin"
    fi
}

install_binary() {
    local os="$1" folder="$2" tag version archive
    tag=$(latest_tag)
    version=${tag#v}
    archive="$(mktemp -d)"

    # The EXIT trap runs after this function returns, so expand the path now.
    # shellcheck disable=SC2064
    trap "rm -rf '$archive'" EXIT

    curl -fsSL "$REPOSITORY/releases/download/$tag/vendor_v${version}_$os.tar.gz" -o "$archive/vendor.tar.gz" ||
        die "could not download vendor $tag"

    # As root, tar would otherwise give the file the owner recorded in the archive.
    tar --no-same-owner -xzf "$archive/vendor.tar.gz" -C "$folder" vendor
}

write_config() {
    cat >"$1" <<EOF
vendorConfig:
  vendorFolder: '$2'
default:
  vendorFolder: '{vendorFolder}'
  locked: false
EOF
}

profile_path() {
    case "${SHELL##*/}" in
        bash)
            if [ "$(uname -s)" = Darwin ]; then
                echo "$HOME/.bash_profile"
            else
                echo "$HOME/.bashrc"
            fi
            ;;
        zsh) echo "${ZDOTDIR:-$HOME}/.zshrc" ;;
        fish) echo "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
        *) echo "$HOME/.profile" ;;
    esac
}

# Writes a path with a literal $HOME, so the profile line survives a copy to another machine.
with_home_variable() {
    case "$1" in
        "$HOME"/*) printf '%s' "\$HOME/${1#"$HOME"/}" ;;
        *) printf '%s' "$1" ;;
    esac
}

# Prints the lines the profile needs, one per line.
profile_lines() {
    local profile="$1" folder="$2" config="$3" config_text folder_text
    config_text=$(with_home_variable "$config")
    folder_text=$(with_home_variable "$folder")

    if [ "${profile##*/}" = config.fish ]; then
        printf 'set -gx DEFAULT_VENDOR_CONFIG "%s"\n' "$config_text"
    else
        printf 'export DEFAULT_VENDOR_CONFIG="%s"\n' "$config_text"
    fi

    case ":$PATH:" in
        *":$folder:"*) ;;
        *)
            if [ "${profile##*/}" = config.fish ]; then
                printf 'fish_add_path "%s"\n' "$folder_text"
            else
                # shellcheck disable=SC2016
                printf 'export PATH="%s:$PATH"\n' "$folder_text"
            fi
            ;;
    esac
}

# Appends each missing line to the profile, and reports what changed.
update_profile() {
    local profile="$1" lines="$2" missing="" separator="" line

    while IFS= read -r line; do
        grep -qxF -- "$line" "$profile" 2>/dev/null || missing+="$line"$'\n'
    done <<<"$lines"

    if [ -z "$missing" ]; then
        printf '%s already has the settings vendor needs:\n' "$profile"
        printf '%s\n' "$lines" | sed 's/^/  /'
        printf '\nOther shells you use need the same settings.\n'
        return
    fi

    # A profile with no final newline would otherwise run our first line into its last one.
    if [ -s "$profile" ] && [ -n "$(tail -c1 "$profile")" ]; then
        separator=$'\n'
    fi

    mkdir -p "$(dirname "$profile")" 2>/dev/null || true
    if printf '%s%s' "$separator" "$missing" 2>/dev/null >>"$profile"; then
        printf 'Added to %s:\n' "$profile"
        printf '%s' "$missing" | sed 's/^/  /'
        printf '\nOther shells you use need the same lines in their config.\n'
        printf 'Restart your shell or run this to start using vendor:\n  source %s\n' "$profile"
    else
        printf 'Could not write to %s. Add these lines to your shell config:\n' "$profile"
        printf '%s' "$missing" | sed 's/^/  /'
    fi
}

main() {
    local os config folder lines profile

    os=$(release_os)
    require_tools

    config="$HOME/vendor.yml"
    if [ -e "$config" ]; then
        folder=$(configured_folder "$config") ||
            die "$config exists but sets no vendorConfig.vendorFolder. Set one, then run this again."
    else
        folder=$(default_folder)
    fi

    mkdir -p "$folder" 2>/dev/null || die "could not create $folder"
    [ -w "$folder" ] || die "$folder is not writable"

    # A binary already in place is left to `vendor add`, which knows its version from the lockfile.
    if [ ! -x "$folder/vendor" ]; then
        install_binary "$os" "$folder"
    fi

    if [ ! -e "$config" ]; then
        write_config "$config" "$folder"
    fi

    "$folder/vendor" --config "$config" --plain add vendorfiles </dev/null

    profile=$(profile_path)
    lines=$(profile_lines "$profile" "$folder" "$config")

    printf '\nvendor is in %s\nIts config is %s\n\n' "$folder" "$config"
    update_profile "$profile" "$lines"
    printf '\nThen install programs with: vendor add <name>\n'
}

main "$@"
