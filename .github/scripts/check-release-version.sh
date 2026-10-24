#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

release_ref=${1:-${GITHUB_REF:-}}
metadata=$(cargo metadata --locked --no-deps --format-version 1)
workspace_version=$(jq -er '
  .packages | map(.version) | unique |
  if length == 1 then .[0]
  else error("workspace packages must share one version")
  end
' <<< "$metadata")

if [[ $release_ref == refs/tags/* ]]; then
  expected_ref="refs/tags/v${workspace_version}"
  if [[ $release_ref != "$expected_ref" ]]; then
    printf 'Release tag %s does not match workspace version %s; expected %s.\n' \
      "${release_ref#refs/tags/}" "$workspace_version" "${expected_ref#refs/tags/}" >&2
    exit 1
  fi
fi

printf 'Workspace packages share version %s.\n' "$workspace_version"
