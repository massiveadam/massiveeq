#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf 'usage: %s DESTINATION\n' "${0##*/}" >&2
  exit 2
fi

repo_root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
destination=$1

if [[ ! -d "$destination" ]]; then
  printf 'destination does not exist: %s\n' "$destination" >&2
  exit 2
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

cp "$repo_root/PKGBUILD" "$scratch/PKGBUILD"
ln -s "$repo_root" "$scratch/massiveeq-git"

current_pkgver=$(
  cd "$scratch"
  # PKGBUILD is project-controlled input. Sourcing it lets the exported AUR
  # metadata use exactly the same pkgver() implementation as makepkg.
  source ./PKGBUILD
  pkgver
)

if [[ ! "$current_pkgver" =~ ^[[:alnum:]_.+]+$ ]]; then
  printf 'pkgver() returned an invalid version: %s\n' "$current_pkgver" >&2
  exit 1
fi

sed "s/^pkgver=.*/pkgver=$current_pkgver/" "$repo_root/PKGBUILD" > "$destination/PKGBUILD"
cp "$repo_root/massiveeq.install" "$destination/massiveeq.install"

(
  cd "$destination"
  makepkg --printsrcinfo > .SRCINFO
)
