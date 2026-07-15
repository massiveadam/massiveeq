# AUR release preparation

The repository root `PKGBUILD` defines the rolling `massiveeq-git` package.
After publishing a signed or annotated GitHub release tag, generate `.SRCINFO`
with:

```sh
makepkg --printsrcinfo > .SRCINFO
```

Publishing to the AUR requires an SSH key registered with an AUR account. Copy
only `PKGBUILD` and `.SRCINFO` into the AUR package repository, review the
result, then commit and push it.

Do not publish a release package until the tagged source has passed the clean
Arch build and the playback/hotplug hardware checks.
