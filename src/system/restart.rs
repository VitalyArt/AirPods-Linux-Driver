//! Relaunch the installed package rather than the old executable/mounted deployment.
use super::updates::Package;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

const FLATPAK_RESTART: &str = include_str!("../../scripts/packaging/restart-flatpak.sh");
const APP_ID: &str = "com.vitalyart.airpods-driver";

fn launch_command(package: &Package, arguments: Vec<OsString>) -> Result<Command, String> {
    let mut command = match package {
        Package::Deb | Package::Rpm => Command::new("/usr/bin/airpods-driver"),
        Package::AppImage(path) => {
            let mut command = Command::new(path);
            // The AppImage runtime must rebuild its environment using the new image.
            for key in [
                "APPDIR",
                "APPIMAGE",
                "ARGV0",
                "OWD",
                "LD_LIBRARY_PATH",
                "LD_PRELOAD",
            ] {
                command.env_remove(key);
            }
            command
        }
        _ => return Err("This installation cannot be restarted directly.".into()),
    };
    command.args(arguments.into_iter().filter(|a| a != "--start-minimized"));
    Ok(command)
}

pub async fn after_update(package: Package) -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os()
        .skip(1)
        .filter(|a| a != "--start-minimized")
        .collect();
    if let Package::Flatpak { installation } = package {
        let info = std::fs::read_to_string("/.flatpak-info").map_err(|e| e.to_string())?;
        let instance = info
            .lines()
            .find_map(|line| line.strip_prefix("instance-id="))
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            .ok_or("Cannot determine the current Flatpak instance")?;
        let mut child = tokio::process::Command::new("flatpak-spawn")
            // No --watch-bus: the helper must survive the old application's exit.
            .args([
                "--host",
                "sh",
                "-c",
                FLATPAK_RESTART,
                "restart-flatpak",
                instance,
                &installation,
                APP_ID,
            ])
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        use tokio::io::AsyncBufReadExt;
        let mut reader =
            tokio::io::BufReader::new(child.stdout.take().ok_or("Restart helper has no output")?);
        let mut ready = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            reader.read_line(&mut ready),
        )
        .await
        .map_err(|_| "Restart helper did not become ready".to_owned())?
        .map_err(|e| e.to_string())?;
        if ready.trim() != "ready" {
            return Err("Could not schedule a Flatpak restart".into());
        }
        // The host helper survives without --watch-bus. Stop the local proxy:
        // otherwise it can keep the old sandbox alive while waiting for the
        // host helper, which in turn is waiting for that sandbox to exit.
        child.kill().await.map_err(|e| e.to_string())?;
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        let mut command = launch_command(&package, arguments)?;
        // exec keeps the PID and replaces all old threads. The single-instance
        // lock is CLOEXEC, so the new program can acquire it without a race.
        Err(format!(
            "Could not start the updated application: {}",
            command.exec()
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exec_child() {
        let Some(directory) = std::env::var_os("LIBREPODS_RESTART_TEST_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let _lock = super::super::single_instance::acquire().unwrap().unwrap();
        std::fs::write(directory.join("old-pid"), std::process::id().to_string()).unwrap();
        let mut command = launch_command(
            &Package::AppImage(directory.join("updated.AppImage")),
            vec!["--debug".into(), "--start-minimized".into()],
        )
        .unwrap();
        panic!("exec failed: {}", command.exec());
    }

    #[test]
    fn replacement_keeps_pid_releases_lock_and_clears_old_image_environment() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("updated.AppImage");
        std::fs::write(&executable, b"#!/bin/sh\nset -eu\ntest -z \"${APPDIR:-}\"\ntest -z \"${LD_LIBRARY_PATH:-}\"\ntest \"$#\" = 1\ntest \"$1\" = --debug\nflock -n \"$XDG_RUNTIME_DIR/airpods-driver.lock\" true\nprintf '%s' \"$$\" > \"$LIBREPODS_RESTART_TEST_DIR/new-pid\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "system::restart::tests::exec_child",
                "--nocapture",
            ])
            .env("LIBREPODS_RESTART_TEST_DIR", directory.path())
            .env("XDG_RUNTIME_DIR", directory.path())
            .env("APPDIR", "/old/mount")
            .env("LD_LIBRARY_PATH", "/old/libraries")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read(directory.path().join("old-pid")).unwrap(),
            std::fs::read(directory.path().join("new-pid")).unwrap()
        );
    }
    #[test]
    fn native_restart_uses_installed_path_and_opens_a_window() {
        for package in [Package::Deb, Package::Rpm] {
            let command =
                launch_command(&package, vec!["--debug".into(), "--start-minimized".into()])
                    .unwrap();
            assert_eq!(command.get_program(), "/usr/bin/airpods-driver");
            assert_eq!(command.get_args().collect::<Vec<_>>(), vec!["--debug"]);
        }
    }
    #[test]
    fn appimage_restart_uses_original_image_without_old_mount_environment() {
        let command = launch_command(
            &Package::AppImage("/home/user/Pods.AppImage".into()),
            vec![],
        )
        .unwrap();
        assert_eq!(command.get_program(), "/home/user/Pods.AppImage");
        for key in ["APPDIR", "APPIMAGE", "LD_LIBRARY_PATH", "LD_PRELOAD"] {
            assert!(command.get_envs().any(|(k, v)| k == key && v.is_none()));
        }
    }
}
