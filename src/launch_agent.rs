use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const LABEL: &str = "com.aabelkhiria.grav-tray-rs";

pub fn path() -> io::Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory not found"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

pub fn is_installed() -> bool {
    path().is_ok_and(|path| path.is_file())
}

pub fn write_for_current_executable() -> io::Result<PathBuf> {
    let executable = env::current_exe()?;
    write(&executable)
}

pub fn write(executable: &Path) -> io::Result<PathBuf> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let executable = xml_escape(&executable.to_string_lossy());
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{executable}</string>
    </array>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
    );
    fs::write(&path, plist)?;
    Ok(path)
}

pub fn remove() -> io::Result<()> {
    let path = path()?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
pub fn install_and_start() -> Result<(), String> {
    let path = write_for_current_executable().map_err(|error| error.to_string())?;
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let bootstrap = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&path)
        .output()
        .map_err(|error| error.to_string())?;
    if bootstrap.status.success() {
        return Ok(());
    }

    let service = format!("{domain}/{LABEL}");
    let kickstart = Command::new("launchctl")
        .args(["kickstart", "-k", &service])
        .output()
        .map_err(|error| error.to_string())?;
    if kickstart.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl could not start Grav Tray.\nbootstrap: {}\nkickstart: {}",
            output_message(&bootstrap),
            output_message(&kickstart)
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn uninstall_and_stop() -> Result<(), String> {
    let service = format!("gui/{}/{}", unsafe { libc::geteuid() }, LABEL);
    let output = Command::new("launchctl")
        .args(["bootout", &service])
        .output()
        .map_err(|error| error.to_string())?;
    // A missing/not-loaded service is already in the desired state.
    if !output.status.success() && is_installed() {
        eprintln!("launchctl: {}", output_message(&output));
    }
    remove().map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn install_and_start() -> Result<(), String> {
    Err("grav-tray-rs is a native macOS menu bar application.".to_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall_and_stop() -> Result<(), String> {
    Err("grav-tray-rs is a native macOS menu bar application.".to_owned())
}

fn output_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("exit status {}", output.status)
    } else {
        stderr
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent_escapes_executable_path() {
        assert_eq!(
            xml_escape("/Applications/Me & You/<tray>"),
            "/Applications/Me &amp; You/&lt;tray&gt;"
        );
    }
}
