#!/usr/bin/env bash

set -euo pipefail

mode=dry-run
case "${1:-}" in
  --check)
    mode=check
    ;;
  --dry-run|'')
    mode=dry-run
    ;;
  --publish)
    mode=publish
    ;;
  *)
    printf 'usage: %s [--check|--dry-run|--publish]\n' "${0##*/}" >&2
    exit 2
    ;;
esac

repo_root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
fetch_url=${AUR_FETCH_URL:-https://aur.archlinux.org/massiveeq-git.git}
push_url=${AUR_PUSH_URL:-ssh://aur@aur.archlinux.org/massiveeq-git.git}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

git clone --quiet "$fetch_url" "$scratch/aur"
mkdir "$scratch/export"
"$repo_root/packaging/aur/export.sh" "$scratch/export"

normalized_copy() {
  local source_dir=$1
  local output_dir=$2

  mkdir -p "$output_dir"
  sed 's/^pkgver=.*/pkgver=__VCS_VERSION__/' \
    "$source_dir/PKGBUILD" > "$output_dir/PKGBUILD"
  sed 's/^[[:space:]]*pkgver = .*/\tpkgver = __VCS_VERSION__/' \
    "$source_dir/.SRCINFO" > "$output_dir/.SRCINFO"
  cp "$source_dir/massiveeq.install" "$output_dir/massiveeq.install"
}

normalized_copy "$scratch/aur" "$scratch/current-normalized"
normalized_copy "$scratch/export" "$scratch/export-normalized"

if diff -qr "$scratch/current-normalized" "$scratch/export-normalized" >/dev/null; then
  printf 'AUR package metadata is already current.\n'
  exit 0
fi

printf 'AUR packaging changes detected:\n'
for file in PKGBUILD .SRCINFO massiveeq.install; do
  diff -u --label "aur/$file" --label "upstream/$file" \
    "$scratch/aur/$file" "$scratch/export/$file" || true
done

if [[ "$mode" == check ]]; then
  # A distinct status lets automation distinguish an update from a real error.
  exit 10
fi

if [[ "$mode" == dry-run ]]; then
  printf 'Dry run only; the AUR repository was not changed.\n'
  exit 0
fi

cp "$scratch/export/PKGBUILD" "$scratch/aur/PKGBUILD"
cp "$scratch/export/.SRCINFO" "$scratch/aur/.SRCINFO"
cp "$scratch/export/massiveeq.install" "$scratch/aur/massiveeq.install"

git -C "$scratch/aur" add PKGBUILD .SRCINFO massiveeq.install
git -C "$scratch/aur" diff --cached --check

if git -C "$scratch/aur" diff --cached --quiet; then
  printf 'AUR package metadata is already current.\n'
  exit 0
fi

git -C "$scratch/aur" config user.name "${AUR_GIT_NAME:-massiveadam}"
git -C "$scratch/aur" config user.email \
  "${AUR_GIT_EMAIL:-massiveadam@users.noreply.github.com}"
git -C "$scratch/aur" commit --quiet -m \
  "${AUR_COMMIT_MESSAGE:-Sync packaging from MassiveEQ upstream}"
git -C "$scratch/aur" remote set-url origin "$push_url"
git -C "$scratch/aur" push origin HEAD:master
