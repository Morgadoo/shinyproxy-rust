#!/usr/bin/env bash
# ShinyProxy license header tool.
#
# Replaces the `mvn license:format` hook of the Java build: every Rust source file must start with the
# Apache-2.0 header defined in LICENSE_HEADER (rendered as a Rust block comment).
#
# Usage:
#   scripts/license-header.sh --check   # exit 1 when a file is missing the header
#   scripts/license-header.sh --fix     # prepend the header where it is missing
set -euo pipefail

MODE="${1:---check}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PROJECT_NAME="ShinyProxy"
INCEPTION_YEAR="2016"
CURRENT_YEAR="$(date +%Y)"

render_header() {
    {
        echo "/*"
        # shellcheck disable=SC2001
        sed \
            -e "s|\${project.name}|${PROJECT_NAME}|g" \
            -e "s|\${project.inceptionYear}|${INCEPTION_YEAR}|g" \
            -e "s|\${year}|${CURRENT_YEAR}|g" \
            LICENSE_HEADER |
            while IFS= read -r line; do
                if [[ -z "$line" ]]; then
                    echo " *"
                else
                    echo " * $line"
                fi
            done
        echo " */"
    }
}

# Include untracked (but not ignored) files so newly created sources are checked too.
mapfile -t FILES < <(git ls-files --cached --others --exclude-standard '*.rs' | sort -u)

MISSING=()
for file in "${FILES[@]}"; do
    # A valid header starts at line 1 and mentions the project and a copyright line.
    if ! head -n 5 "$file" | grep -q "Copyright (C) ${INCEPTION_YEAR}-[0-9]\{4\} Open Analytics"; then
        MISSING+=("$file")
    fi
done

if [[ "$MODE" == "--check" ]]; then
    if [[ ${#MISSING[@]} -gt 0 ]]; then
        echo "Missing license header in ${#MISSING[@]} file(s):" >&2
        printf '  %s\n' "${MISSING[@]}" >&2
        echo "Run scripts/license-header.sh --fix" >&2
        exit 1
    fi
    echo "License headers OK (${#FILES[@]} files checked)"
    exit 0
fi

if [[ "$MODE" != "--fix" ]]; then
    echo "usage: $0 [--check|--fix]" >&2
    exit 2
fi

HEADER="$(render_header)"
for file in "${MISSING[@]}"; do
    tmp="$(mktemp)"
    printf '%s\n\n' "$HEADER" >"$tmp"
    cat "$file" >>"$tmp"
    mv "$tmp" "$file"
    echo "added header: $file"
done
echo "Added license header to ${#MISSING[@]} file(s)"
