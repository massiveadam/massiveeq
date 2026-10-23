//! Noticing a package upgrade under a running process.

use std::path::Path;

/// Exit status if re-executing fails; systemd's `Restart=on-failure` then
/// starts the service on the new binaries instead.
pub const UPGRADE_RESTART_EXIT: i32 = 75;

/// Whether this process's executable was replaced on disk, as pacman does
/// when it upgrades a package: the running process is then the old version.
pub fn executable_replaced() -> bool {
    std::fs::read_link("/proc/self/exe").is_ok_and(|path| replaced_link(&path))
}

/// Replace this process with the newly installed version of itself, with the
/// same arguments, whatever started it (systemd, D-Bus, autostart, a shell).
/// Returns only on failure; the caller should then exit with
/// [`UPGRADE_RESTART_EXIT`].
pub fn exec_new_version() -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut arguments = std::env::args_os();
    let program = arguments.next().unwrap_or_default();
    std::process::Command::new(program).args(arguments).exec()
}

fn replaced_link(path: &Path) -> bool {
    path.to_string_lossy().ends_with(" (deleted)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deleted_executable_link_means_an_upgrade() {
        assert!(replaced_link(Path::new("/usr/bin/massiveeqd (deleted)")));
        assert!(!replaced_link(Path::new("/usr/bin/massiveeqd")));
        assert!(!executable_replaced());
    }
}
