use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=MASSIVEEQ_BUILD_COMMIT");
    watch_git_revision();

    let commit = env::var("MASSIVEEQ_BUILD_COMMIT")
        .ok()
        .filter(|value| valid_commit(value))
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=MASSIVEEQ_BUILD_COMMIT={commit}");
}

fn watch_git_revision() {
    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        return;
    };
    let head = Command::new("git")
        .arg("-C")
        .arg(&manifest_dir)
        .args(["rev-parse", "--path-format=absolute", "--git-path", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    if let Some(head) = head {
        println!("cargo:rerun-if-changed={}", head.trim());
    }

    let reference = Command::new("git")
        .arg("-C")
        .arg(&manifest_dir)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    let Some(reference) = reference else {
        return;
    };
    let reference_path = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            reference.trim(),
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    if let Some(reference_path) = reference_path {
        println!("cargo:rerun-if-changed={}", reference_path.trim());
    }
}

fn git_commit() -> Option<String> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")?;
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    valid_commit(value).then(|| value.to_owned())
}

fn valid_commit(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
