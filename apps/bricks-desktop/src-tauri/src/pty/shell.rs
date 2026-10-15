//! The program a terminal starts, per platform.

/// Program and arguments of a terminal's shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// The user's shell: `$SHELL`, else the first of zsh, bash, sh that exists,
/// started as a login interactive shell.
#[cfg(unix)]
pub fn default_shell() -> ShellSpec {
    let env_shell = std::env::var("SHELL").ok();
    pick_unix_shell(env_shell.as_deref(), |path| {
        std::path::Path::new(path).is_file()
    })
}

/// `%COMSPEC%` (cmd.exe unless changed), else PowerShell.
#[cfg(windows)]
pub fn default_shell() -> ShellSpec {
    let comspec = std::env::var("COMSPEC").ok();
    pick_windows_shell(comspec.as_deref())
}

#[cfg(any(unix, test))]
fn pick_unix_shell(env_shell: Option<&str>, exists: impl Fn(&str) -> bool) -> ShellSpec {
    let program = env_shell
        .filter(|path| !path.is_empty() && exists(path))
        .map(str::to_string)
        .or_else(|| {
            ["/bin/zsh", "/bin/bash", "/bin/sh"]
                .into_iter()
                .find(|path| exists(path))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "/bin/sh".to_string());
    ShellSpec {
        program,
        args: vec!["-l".to_string(), "-i".to_string()],
    }
}

#[cfg(any(windows, test))]
fn pick_windows_shell(comspec: Option<&str>) -> ShellSpec {
    let program = comspec
        .filter(|s| !s.is_empty())
        .unwrap_or("powershell.exe")
        .to_string();
    ShellSpec { program, args: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_prefers_existing_shell_from_environment() {
        let spec = pick_unix_shell(Some("/opt/bin/fish"), |p| p == "/opt/bin/fish");
        assert_eq!(spec.program, "/opt/bin/fish");
        assert_eq!(spec.args, vec!["-l", "-i"]);
    }

    #[test]
    fn unix_falls_back_when_environment_shell_is_missing() {
        let spec = pick_unix_shell(Some("/nope/shell"), |p| p == "/bin/bash" || p == "/bin/sh");
        assert_eq!(spec.program, "/bin/bash");
        let spec = pick_unix_shell(None, |p| p == "/bin/sh");
        assert_eq!(spec.program, "/bin/sh");
        let spec = pick_unix_shell(Some(""), |_| false);
        assert_eq!(spec.program, "/bin/sh");
    }

    #[test]
    fn windows_uses_comspec_then_powershell() {
        assert_eq!(pick_windows_shell(Some("C:\\Windows\\system32\\cmd.exe")).program, "C:\\Windows\\system32\\cmd.exe");
        assert_eq!(pick_windows_shell(None).program, "powershell.exe");
        assert_eq!(pick_windows_shell(Some("")).program, "powershell.exe");
    }
}
