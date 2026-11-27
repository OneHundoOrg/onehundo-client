use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns the standard XDG autostart `.desktop` path (`~/.config/autostart/onehundo.desktop`).
pub fn default_autostart_file_path() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg.trim())
                .join("autostart")
                .join("onehundo.desktop");
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim())
                .join(".config")
                .join("autostart")
                .join("onehundo.desktop");
        }
    }
    PathBuf::from("/tmp/onehundo-autostart.desktop")
}

/// Resolves the stable launch executable: `$APPIMAGE` when running from an AppImage so autostart
/// does not reference a temporary `/tmp/.mount_*` path, otherwise `current_exe()`.
pub fn preferred_launch_executable() -> PathBuf {
    if let Ok(appimage) = env::var("APPIMAGE") {
        let trimmed = appimage.trim();
        if !trimmed.is_empty() {
            let candidate = PathBuf::from(trimmed);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    env::current_exe().unwrap_or_else(|_| PathBuf::from("onehundo-desktop"))
}

pub fn is_autostart_enabled(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let mut hidden = false;
    let mut x_gnome_enabled = true;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("Hidden=") {
            hidden = val.trim().eq_ignore_ascii_case("true");
        } else if let Some(val) = trimmed.strip_prefix("X-GNOME-Autostart-enabled=") {
            x_gnome_enabled = val.trim().eq_ignore_ascii_case("true");
        }
    }
    !hidden && x_gnome_enabled
}

fn quote_desktop_exec_arg(path: &Path) -> String {
    let raw = path.display().to_string();
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

pub fn render_autostart_desktop_entry(exec_path: &Path, minimized: bool) -> String {
    let mut exec = quote_desktop_exec_arg(exec_path);
    if minimized {
        exec.push_str(" --minimized");
    }
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=OneHundo\n\
         GenericName=Achievement Tracker\n\
         Comment=Automatic achievement tracking across emulators, ports, and launchers\n\
         Exec={}\n\
         Icon=onehundo\n\
         Terminal=false\n\
         Categories=Game;Utility;\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec
    )
}

pub fn set_autostart_enabled(
    path: &Path,
    enabled: bool,
    exec_path: &Path,
    minimized: bool,
) -> Result<bool, String> {
    if !enabled {
        if path.exists() {
            fs::remove_file(path).map_err(|e| {
                format!("failed removing autostart entry '{}': {}", path.display(), e)
            })?;
        }
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed creating autostart directory '{}': {}",
                    parent.display(),
                    e
                )
            })?;
        }
    }
    let contents = render_autostart_desktop_entry(exec_path, minimized);
    fs::write(path, contents)
        .map_err(|e| format!("failed writing autostart entry '{}': {}", path.display(), e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn autostart_enable_disable_and_quoting() {
        let dir = env::temp_dir().join(format!(
            "onehundo-autostart-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let desktop_path = dir.join("autostart").join("onehundo.desktop");
        assert!(!is_autostart_enabled(&desktop_path));

        let appimage_path = PathBuf::from("/home/player/My Apps/OneHundo-x86_64.AppImage");
        let enabled = set_autostart_enabled(&desktop_path, true, &appimage_path, true)
            .expect("enable autostart");
        assert!(enabled);
        assert!(is_autostart_enabled(&desktop_path));

        let raw = fs::read_to_string(&desktop_path).unwrap();
        assert!(raw.contains("Exec=\"/home/player/My Apps/OneHundo-x86_64.AppImage\" --minimized"));
        assert!(raw.contains("X-GNOME-Autostart-enabled=true"));

        let disabled = set_autostart_enabled(&desktop_path, false, &appimage_path, true)
            .expect("disable autostart");
        assert!(!disabled);
        assert!(!is_autostart_enabled(&desktop_path));
        assert!(!desktop_path.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
