//! Per-user XDG desktop autostart, including the host session for Flatpak.
use std::{env, fs, path::PathBuf};

const FILE: &str = "com.vitalyart.airpods-driver.desktop";
const HOST: &str = r#"
set -eu
config=${XDG_CONFIG_HOME:-"$HOME/.config"}
case "$config" in /*) ;; *) config="$HOME/.config" ;; esac
file="$config/autostart/com.vitalyart.airpods-driver.desktop"
case "$1" in
read) if [ -f "$file" ]; then cat "$file"; fi ;;
write) mkdir -p "$config/autostart"; printf '%s' "$2" > "$file" ;;
esac
"#;

fn path() -> Result<PathBuf, String> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or("Cannot find the user configuration directory")?;
    Ok(config.join("autostart").join(FILE))
}

fn host(action: &str, contents: &str) -> Result<String, String> {
    let output = std::process::Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", HOST, "autostart", action, contents])
        .output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn flatpak() -> bool { std::path::Path::new("/.flatpak-info").exists() }

fn contents() -> Result<String, String> {
    let contents = if flatpak() {
        host("read", "")?
    } else {
        match fs::read_to_string(path()?) {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(contents)
}

pub fn enabled() -> Result<bool, String> {
    Ok(is_enabled(&contents()?))
}

/// Run this migration once, independent of the installed application version.
pub fn initialize() -> Result<bool, String> {
    let marker = crate::utils::get_preferences_path()
        .with_file_name("autostart-initialized");
    initialize_once(&marker, || {
        let existing = contents()?;
        if existing.is_empty() { set_enabled(true) } else { Ok(is_enabled(&existing)) }
    })?;
    enabled()
}

fn initialize_once(
    marker: &std::path::Path,
    migrate: impl FnOnce() -> Result<bool, String>,
) -> Result<(), String> {
    match fs::metadata(marker) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e.to_string()),
    }
    migrate()?;
    fs::create_dir_all(marker.parent().ok_or("Invalid migration marker path")?)
        .map_err(|e| e.to_string())?;
    fs::write(marker, "1\n").map_err(|e| e.to_string())
}

fn is_enabled(contents: &str) -> bool {
    let mut entry = false;
    let mut executable = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') { entry = line == "[Desktop Entry]"; }
        if !entry { continue; }
        if line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false" { return false; }
        executable |= line.starts_with("Exec=");
    }
    executable
}

fn quote(value: &str) -> Result<String, String> {
    if value.contains(['\n', '\r', '\0']) { return Err("Invalid executable path".into()); }
    let mut result = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => result.push_str("\\\\\\\\"),
            '"' | '`' | '$' => { result.push_str("\\\\"); result.push(c); }
            '%' => result.push_str("%%"),
            _ => result.push(c),
        }
    }
    result.push('"');
    Ok(result)
}

pub fn set_enabled(value: bool) -> Result<bool, String> {
    let contents = if value {
        let command = if flatpak() {
            "flatpak run com.vitalyart.airpods-driver".to_owned()
        } else {
            let executable = env::var_os("APPIMAGE").map(PathBuf::from)
                .map(Ok).unwrap_or_else(env::current_exe).map_err(|e| e.to_string())?;
            if !executable.is_absolute() { return Err("Executable path must be absolute".into()); }
            quote(executable.to_str().ok_or("Executable path is not UTF-8")?)?
        };
        format!("[Desktop Entry]\nType=Application\nName=Airpods Driver\nExec={command} --start-minimized\nIcon=com.vitalyart.airpods-driver\nTerminal=false\n")
    } else {
        // Override any system-wide entry with the same name as well.
        "[Desktop Entry]\nType=Application\nName=Airpods Driver\nHidden=true\n".into()
    };
    if flatpak() { host("write", &contents)?; } else {
        let path = path()?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(path, contents).map_err(|e| e.to_string())?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_runs_only_once_and_retries_failures() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("config/autostart-initialized");
        assert!(initialize_once(&marker, || Err("write failed".into())).is_err());
        assert!(!marker.exists());
        initialize_once(&marker, || Ok(true)).unwrap();
        initialize_once(&marker, || panic!("must preserve user choice after updates")).unwrap();
    }
    #[test]
    fn host_helper_uses_host_config_and_round_trips_entries() {
        let directory = tempfile::tempdir().unwrap();
        let run = |action: &str, contents: &str| {
            let output = std::process::Command::new("sh")
                .args(["-c", HOST, "autostart", action, contents])
                .env("XDG_CONFIG_HOME", directory.path().join("config with spaces"))
                .output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(run("read", ""), "");
        let entry = "[Desktop Entry]\nExec=app --start-minimized\n";
        run("write", entry);
        assert_eq!(run("read", ""), entry);
        assert!(is_enabled(&run("read", "")));
        run("write", "[Desktop Entry]\nHidden=true\n");
        assert!(!is_enabled(&run("read", "")));
    }
    #[test]
    fn respects_disabled_entries_and_sections() {
        assert!(!is_enabled(""));
        assert!(is_enabled("[Desktop Entry]\nExec=app\n"));
        assert!(!is_enabled("[Desktop Entry]\nExec=app\nHidden=true\n"));
        assert!(!is_enabled("[Desktop Entry]\nExec=app\nX-GNOME-Autostart-enabled=false\n"));
        assert!(!is_enabled("[Desktop Action test]\nExec=app\n"));
    }
    #[test]
    fn escapes_desktop_exec_paths() {
        assert_eq!(quote("/tmp/A B%\\\"$`" ).unwrap(), "\"/tmp/A B%%\\\\\\\\\\\\\"\\\\$\\\\`\"");
        assert!(quote("/tmp/a\nb").is_err());
    }
}
