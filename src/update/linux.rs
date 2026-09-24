//! Linux install: unpack the release tarball and replace the binary after exit.

#![cfg(any(test, target_os = "linux"))]

use std::path::Path;

#[cfg(target_os = "linux")]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

#[cfg(target_os = "linux")]
use super::{updater_paths, wait_for_updater_ready, UpdateError};

#[cfg(target_os = "linux")]
pub(super) fn schedule(downloaded: &Path) -> Result<(), UpdateError> {
    let current = std::env::current_exe()
        .map_err(|error| UpdateError::new(format!("Could not locate the running app: {error}")))?;
    let current = current.canonicalize().unwrap_or(current);
    ensure_install_dir_writable(&current)?;

    let paths = updater_paths("sh");
    let _ = fs::remove_file(&paths.ready);

    let script_text = update_script(
        std::process::id(),
        downloaded,
        &current,
        &paths.ready,
        &paths.log,
        &paths.script,
    );
    fs::write(&paths.script, script_text)
        .map_err(|error| UpdateError::new(format!("Could not prepare updater: {error}")))?;
    fs::set_permissions(&paths.script, fs::Permissions::from_mode(0o700))
        .map_err(|error| UpdateError::new(format!("Could not prepare updater: {error}")))?;

    let mut child = Command::new("/bin/sh")
        .arg(&paths.script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| UpdateError::new(format!("Could not start updater: {error}")))?;

    wait_for_updater_ready(&mut child, &paths.ready, &paths.script, &paths.log)
}

/// Fail before handoff when the install location is not writable.
#[cfg(target_os = "linux")]
fn ensure_install_dir_writable(current: &Path) -> Result<(), UpdateError> {
    let parent = current.parent().ok_or_else(|| {
        UpdateError::new("Could not determine the install directory for PinkDown")
    })?;
    let probe = parent.join(format!(".pinkdown-update-write-test-{}", std::process::id()));
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(UpdateError::new(format!(
            "Cannot write to {}: {error}. A system-wide install needs a manual \
             upgrade (re-run installer/linux/install.sh), or install PinkDown \
             for your user to enable in-app updates.",
            parent.display()
        ))),
    }
}

/// POSIX sh helper: wait for exit, unpack the tarball, atomically replace the
/// binary, relaunch.
pub(super) fn update_script(
    parent_id: u32,
    tarball: &Path,
    current: &Path,
    ready: &Path,
    log: &Path,
    script: &Path,
) -> String {
    let quote = |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    [
        "#!/bin/sh".to_owned(),
        "set -u".to_owned(),
        format!("parent_id={parent_id}"),
        format!("tarball={}", quote(tarball)),
        format!("current={}", quote(current)),
        format!("ready={}", quote(ready)),
        format!("log={}", quote(log)),
        format!("script={}", quote(script)),
        "staging=\"\"".to_owned(),
        "cleanup() {".to_owned(),
        "  if [ -n \"${staging}\" ] && [ -e \"${staging}\" ]; then".to_owned(),
        "    rm -rf \"${staging}\"".to_owned(),
        "  fi".to_owned(),
        "  rm -f \"${ready}\" \"${script}\"".to_owned(),
        "}".to_owned(),
        "trap cleanup EXIT".to_owned(),
        "printf 'ready\\n' > \"${ready}\"".to_owned(),
        "if kill -0 \"${parent_id}\" 2>/dev/null; then".to_owned(),
        "  while kill -0 \"${parent_id}\" 2>/dev/null; do sleep 0.25; done".to_owned(),
        "fi".to_owned(),
        "sleep 0.5".to_owned(),
        "{".to_owned(),
        "  staging=$(mktemp -d \"${TMPDIR:-/tmp}/pinkdown-update.XXXXXX\")".to_owned(),
        "  tar -xzf \"${tarball}\" -C \"${staging}\"".to_owned(),
        "  src=$(find \"${staging}\" -maxdepth 2 -type f -name pinkdown | head -n 1)".to_owned(),
        "  if [ -z \"${src}\" ]; then".to_owned(),
        "    echo 'pinkdown binary not found in release package' >&2".to_owned(),
        "    exit 1".to_owned(),
        "  fi".to_owned(),
        "  chmod 755 \"${src}\"".to_owned(),
        // Stage beside the live binary, then rename so a crash mid-copy cannot
        // leave a half-written pinkdown.
        "  staged=\"${current}.new\"".to_owned(),
        "  rm -f \"${staged}\"".to_owned(),
        "  cp \"${src}\" \"${staged}\"".to_owned(),
        "  mv -f \"${staged}\" \"${current}\"".to_owned(),
        "  rm -rf \"${staging}\"".to_owned(),
        "  staging=\"\"".to_owned(),
        "  rm -f \"${tarball}\"".to_owned(),
        "  \"${current}\" >/dev/null 2>&1 &".to_owned(),
        "  rm -f \"${log}\"".to_owned(),
        "} >\"${log}\" 2>&1 || {".to_owned(),
        "  if [ -x \"${current}\" ]; then \"${current}\" >/dev/null 2>&1 & fi".to_owned(),
        "  exit 1".to_owned(),
        "}".to_owned(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::update_script;
    use std::path::Path;

    #[test]
    fn installer_script_waits_then_replaces_the_binary() {
        let script = update_script(
            42,
            Path::new("pinkdown-linux-x86_64.tar.gz"),
            Path::new("/home/user/.local/bin/pinkdown"),
            Path::new("ready"),
            Path::new("error.log"),
            Path::new("update.sh"),
        );
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("kill -0 \"${parent_id}\""));
        assert!(script.contains("tar -xzf \"${tarball}\" -C \"${staging}\""));
        assert!(script.contains("cp \"${src}\" \"${staged}\""));
        assert!(script.contains("mv -f \"${staged}\" \"${current}\""));
        assert!(script.contains("\"${current}\" >/dev/null 2>&1 &"));
        assert!(script.contains("rm -f \"${tarball}\""));
    }

    /// End-to-end: the generated script really unpacks a release tarball and
    /// swaps the installed binary once the parent process is gone.
    #[cfg(target_os = "linux")]
    #[test]
    fn installer_script_replaces_the_installed_binary() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};

        let root = std::env::temp_dir().join(format!("pinkdown-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();

        let shipped = package.join("pinkdown");
        fs::write(&shipped, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&shipped, fs::Permissions::from_mode(0o755)).unwrap();

        let tarball = root.join("pinkdown-linux-x86_64.tar.gz");
        let status = Command::new("tar")
            .args(["-czf"])
            .arg(&tarball)
            .args(["-C"])
            .arg(&package)
            .arg("pinkdown")
            .status()
            .unwrap();
        assert!(status.success());

        let current = root.join("bin").join("pinkdown");
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        fs::write(&current, b"#!/bin/sh\nexit 0\nOLD\n").unwrap();
        fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).unwrap();

        let ready = root.join("ready");
        let log = root.join("error.log");
        let script = root.join("update.sh");

        // A parent that has already exited: the helper proceeds immediately.
        let mut parent = Command::new("sleep").arg("30").spawn().unwrap();
        let parent_id = parent.id();
        parent.kill().unwrap();
        parent.wait().unwrap();

        let text = update_script(parent_id, &tarball, &current, &ready, &log, &script);
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

        let output = Command::new("/bin/sh").arg(&script).output().unwrap();
        assert!(
            output.status.success(),
            "updater failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        assert_eq!(fs::read(&current).unwrap(), b"#!/bin/sh\nexit 0\n");
        assert!(!tarball.exists(), "downloaded package should be cleaned up");
        assert!(!ready.exists(), "ready marker should be cleaned up");
        assert!(!script.exists(), "updater script should be cleaned up");
        assert!(!log.exists(), "log should be removed on success");
        let _ = fs::remove_dir_all(&root);
    }
}
