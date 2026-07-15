# AUR release preparation

The repository root `PKGBUILD` defines the rolling `massiveeq-git` package.
MassiveEQ 0.3.0-beta.1 is a public beta. The package description and upstream README
must retain that designation until the project is declared stable.

Before every AUR push:

1. Confirm that neither the official repositories nor the AUR already provide
   the same package.
2. Review the complete AUR submission rules, package guidelines, Rust package
   guidelines, and VCS package guidelines.
3. Build and test the package in a clean, fully updated Arch environment.
4. Run `namcap PKGBUILD` and `namcap` on the resulting package archive; resolve
   every actionable error and warning.
5. Inspect the archive contents and dependency metadata.
6. Complete the playback, bypass/fail-open, live-edit, and hotplug hardware
   checks documented upstream.
7. Regenerate `.SRCINFO` from the final `PKGBUILD`:

```sh
makepkg --printsrcinfo > .SRCINFO
```

8. Copy only `PKGBUILD`, `.SRCINFO`, and referenced packaging support files
   such as `massiveeq.install` into the AUR Git repository. Never upload built
   packages, binaries, or source archives.
9. Review the exact diff and commit history before pushing over SSH.

Expected `namcap` exceptions must be reviewed rather than blindly suppressed.
`pipewire` supplies the `pw-dump` and `pw-metadata` commands used at runtime;
`wireplumber` supplies `wpctl` and the active session policy. Static analysis
cannot see those subprocess and service dependencies. A warning that the ELF
interpreter itself is an unused shared library is also a `namcap` false
positive. No other warnings or errors are accepted for publication.

The currently documented hardware coverage is Arch Linux x86_64 on niri with
Bluetooth A2DP and analog stereo outputs. Do not advertise GNOME, KDE Plasma,
surround, ARM, or other untested configurations as validated.
