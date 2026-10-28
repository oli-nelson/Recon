#!/usr/bin/env bash
# Shared version contract for dispatch, changelog promotion, and packaging.
release_version() {
  local value=${1-}
  value=${value#v}
  local number='(0|[1-9][0-9]{0,3})'
  local pattern="^${number}\\.${number}\\.${number}(-(alpha|beta|rc)\\.(0|[1-9][0-9]*))?$"
  if [[ $# != 1 || ! $value =~ $pattern ]]; then
    echo 'error: use X.Y.Z or X.Y.Z-{alpha,beta,rc}.N (optional v prefix; components 0-9999, no leading zeroes)' >&2
    return 1
  fi
  printf '%s\n' "$value"
}
