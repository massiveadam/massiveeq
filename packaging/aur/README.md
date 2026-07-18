# AUR publishing

The repository root `PKGBUILD` defines the rolling `massiveeq-git` package.
MassiveEQ 0.3.0-beta.2 is a public beta. The package description and upstream README
must retain that designation until the project is declared stable.

## Automatic updates

The `Sync AUR` GitHub Actions workflow runs after a successful `CI` push build
on `main`. It exports the current `PKGBUILD`, calculates its VCS version,
regenerates `.SRCINFO`, and compares the result with the live
`massiveeq-git` AUR repository. Packaging changes are committed and pushed;
ordinary upstream code commits are intentionally ignored because AUR helpers
already fetch the newest commit when rebuilding a `-git` package.

The publisher requires one repository secret:

1. Create a dedicated, unencrypted automation key. Do not reuse a personal SSH
   key:

   ```sh
   ssh-keygen -t ed25519 -C 'MassiveEQ AUR automation' \
     -f ~/.ssh/massiveeq_aur_actions -N ''
   ```

2. Add the contents of `~/.ssh/massiveeq_aur_actions.pub` to the **SSH Public
   Key** field in the `massiveadam` AUR account.
3. Add the private half to the GitHub repository as the Actions secret
   `AUR_SSH_PRIVATE_KEY`:

   ```sh
   gh secret set AUR_SSH_PRIVATE_KEY < ~/.ssh/massiveeq_aur_actions
   ```

The key is only loaded when a real packaging difference has been found. The
workflow verifies the AUR's published Ed25519 host-key fingerprint before it
connects. Revoke the dedicated key in the AUR account to immediately disable
automated publishing.

Use **Actions → Sync AUR → Run workflow** with publishing disabled to review a
dry run. Enable the `publish` option only to retry or deliberately force the
normal publish step from `main`.

The same comparison is available locally:

```sh
packaging/aur/sync.sh --dry-run
```

`packaging/aur/sync.sh --publish` performs a real push and therefore requires
working AUR SSH credentials. It never builds or uploads a binary package.

## Manual release checks

Before merging any packaging change that will trigger an AUR push:

1. Confirm that the official repositories do not provide the same package and
   that `massiveeq-git` is still maintained by the expected AUR account.
2. Review the complete AUR submission rules, package guidelines, Rust package
   guidelines, and VCS package guidelines.
3. Build and test the package in a clean, fully updated Arch environment.
4. Run `namcap PKGBUILD` and `namcap` on the resulting package archive; resolve
   every actionable error and warning.
5. Inspect the archive contents and dependency metadata.
6. Complete the playback, bypass/fail-open, live-edit, and hotplug hardware
   checks documented upstream.
7. Verify the repository `.SRCINFO` matches the final `PKGBUILD`:

```sh
makepkg --printsrcinfo > .SRCINFO
```

8. Review the exact workflow diff and commit history before pushing over SSH.
   Never upload built packages, binaries, or source archives.

Expected `namcap` exceptions must be reviewed rather than blindly suppressed.
`pipewire` supplies the `pw-dump` and `pw-metadata` commands used at runtime;
`wireplumber` supplies `wpctl` and the active session policy. Static analysis
cannot see those subprocess and service dependencies. A warning that the ELF
interpreter itself is an unused shared library is also a `namcap` false
positive. No other warnings or errors are accepted for publication.

The currently documented hardware coverage is Arch Linux x86_64 on niri with
Bluetooth A2DP and analog stereo outputs. Do not advertise GNOME, KDE Plasma,
surround, ARM, or other untested configurations as validated.
