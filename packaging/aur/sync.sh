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

# Every new commit on main is published with its VCS version, so update
# checkers that only compare AUR versions (cachy-update, arch-update, and AUR
# helpers without devel checks) see a new MassiveEQ like any other package.
srcinfo_version() {
  sed -n 's/^[[:space:]]*pkgver = //p' "$1/.SRCINFO" | head -n 1
}
current_version=$(srcinfo_version "$scratch/aur")
export_version=$(srcinfo_version "$scratch/export")

if [[ -n "$current_version" ]] && (( $(vercmp "$export_version" "$current_version") < 0 )); then
  # A shallow checkout counts one commit; never publish a version that
  # sorts below what users already have.
  printf 'Refusing to publish %s below the AUR version %s.\n' \
    "$export_version" "$current_version" >&2
  exit 1
fi

unchanged=true
for file in PKGBUILD .SRCINFO massiveeq.install; do
  cmp -s "$scratch/aur/$file" "$scratch/export/$file" || unchanged=false
done
if [[ "$unchanged" == true ]]; then
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
  "${AUR_COMMIT_MESSAGE:-Update to $export_version}"
git -C "$scratch/aur" remote set-url origin "$push_url"
git -C "$scratch/aur" push origin HEAD:master
