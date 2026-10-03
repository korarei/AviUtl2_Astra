use crate::config::{Config, TaskCall};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::io::Write;
use std::process::Command;

pub(crate) fn run(target: &TaskCall, config: &Config, is_root: bool) -> anyhow::Result<()> {
    let mut active_tasks = Vec::new();
    dispatch(target, config, is_root, &mut active_tasks)
}

fn dispatch(call: &TaskCall, config: &Config, is_root: bool, active_tasks: &mut Vec<String>) -> anyhow::Result<()> {
    let task = config.task(call)?;
    let id = task.id().to_owned();

    if is_root && task.hide() {
        bail!("cannot run hidden task '{id}'");
    }

    if let Some(st) = active_tasks.iter().position(|name| name == &id) {
        let mut cycle = active_tasks[st..].to_vec();
        cycle.push(id);
        bail!("circular task dependency: {}", cycle.join(" -> "));
    }

    tracing::info!("Running task '{id}'");
    active_tasks.push(id);

    let result = (|| {
        for dep in task.depends() {
            tracing::debug!("calling dependency '{}'", dep.id());

            dispatch(dep, config, false, active_tasks)?;
        }

        let mut err = None;

        if !task.run().is_empty() {
            for cmd in task.run() {
                let result = if let Some(shell) = task.shell() {
                    exec_script(cmd, shell, task.env())
                } else {
                    exec_default(cmd, task.env())
                };

                if let Err(e) = result {
                    err = Some(e);
                    break;
                }
            }
        }

        for dep in task.finally() {
            tracing::debug!("calling finally task '{}'", dep.id());

            if let Err(e) = dispatch(dep, config, false, active_tasks) {
                err = Some(match err {
                    Some(prev) => prev.context(format!("finally task '{}' failed: {e}", dep.id())),
                    None => e,
                });
            }
        }

        err.map_or(Ok(()), Err)
    })();

    let _ = active_tasks.pop();

    result
}

fn exec_default(cmd: &str, env: &IndexMap<String, String>) -> anyhow::Result<()> {
    tracing::info!("$ {cmd}");

    let mut process = if cfg!(windows) {
        let mut p = Command::new("cmd");
        p.args(["/d", "/s", "/c"]).arg(cmd);
        p
    } else {
        let mut p = Command::new("sh");
        p.args(["-e", "-c"]).arg(cmd);
        p
    };

    process.envs(env);

    let status = process
        .status()
        .with_context(|| format!("failed to execute command '{cmd}'"))?;

    if !status.success() {
        bail!("command '{cmd}' failed with status: {status}");
    }

    tracing::debug!("command '{cmd}' finished with status: {status}");

    Ok(())
}

fn exec_script(cmd: &str, shell: &str, env: &IndexMap<String, String>) -> anyhow::Result<()> {
    let shell_exe = env
        .iter()
        .find(|(k, _)| {
            if cfg!(windows) {
                k.eq_ignore_ascii_case("path")
            } else {
                *k == "PATH"
            }
        })
        .and_then(|(_, path)| which::which_in(shell, Some(path), ".").ok())
        .or_else(|| which::which(shell).ok())
        .with_context(|| format!("failed to find shell '{shell}'"))?;

    let name = shell_exe
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("failed to determine shell name from '{}'", shell_exe.display()))?
        .to_string_lossy()
        .to_lowercase();
    let runner = Runner::from_name(&name).ok_or_else(|| anyhow::anyhow!("shell '{shell}' is not supported"))?;

    let script = runner.render(cmd);

    let mut file = tempfile::Builder::new()
        .prefix("astra_task_")
        .suffix(runner.suffix())
        .tempfile()
        .with_context(|| {
            format!(
                "failed to create temporary script in '{}'",
                std::env::temp_dir().display()
            )
        })?;

    tracing::debug!("generated temporary script '{}'", file.path().display());

    runner
        .write(&mut file, &script)
        .with_context(|| format!("failed to write '{}'", file.path().display()))?;

    let path = file.into_temp_path();

    tracing::info!("$ {shell} {} {}", runner.args().join(" "), path.display());
    tracing::debug!("script content:\n{script}");

    let mut process = Command::new(&shell_exe);
    process.args(runner.args());
    process.arg(&path);

    process.envs(env);

    let status = process
        .status()
        .with_context(|| format!("failed to execute script with '{shell}'"))?;

    if !status.success() {
        bail!("command '{cmd}' failed with status: {status}");
    }

    tracing::debug!("script '{shell}' finished with status: {status}");

    Ok(())
}

#[derive(Clone, Copy)]
enum Runner {
    #[cfg(windows)]
    Cmd,
    #[cfg(windows)]
    PowerShell,
    Pwsh,
    Bash,
    Zsh,
    Fish,
    Csh,
    Nu,
    Sh,
}

impl Runner {
    fn from_name(name: &str) -> Option<Self> {
        #[cfg(windows)]
        if name == "cmd" {
            return Some(Self::Cmd);
        }

        #[cfg(windows)]
        if name == "powershell" {
            return Some(Self::PowerShell);
        }

        if name == "pwsh" {
            Some(Self::Pwsh)
        } else if name.ends_with("bash") {
            Some(Self::Bash)
        } else if name.ends_with("zsh") {
            Some(Self::Zsh)
        } else if name == "fish" {
            Some(Self::Fish)
        } else if name == "csh" || name == "tcsh" {
            Some(Self::Csh)
        } else if name == "nu" {
            Some(Self::Nu)
        } else if name == "sh" || name == "dash" || name == "ash" || name == "ksh" {
            Some(Self::Sh)
        } else {
            None
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            #[cfg(windows)]
            Self::Cmd => &["/d", "/e:on", "/v:off", "/s", "/c"],
            #[cfg(windows)]
            Self::PowerShell => &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"],
            Self::Pwsh => &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"],
            Self::Bash => &["--noprofile", "--norc", "-euo", "pipefail"],
            Self::Zsh => &["-f", "-euo", "pipefail"],
            Self::Fish => &["--no-config"],
            Self::Csh => &["-f", "-e"],
            Self::Nu => &["--no-config-file"],
            Self::Sh => &["-eu"],
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            #[cfg(windows)]
            Self::Cmd => ".bat",
            #[cfg(windows)]
            Self::PowerShell => ".ps1",
            Self::Pwsh => ".ps1",
            Self::Bash | Self::Zsh | Self::Sh => ".sh",
            Self::Fish => ".fish",
            Self::Csh => ".csh",
            Self::Nu => ".nu",
        }
    }

    fn render(self, cmd: &str) -> String {
        let lines = cmd.lines().map(|s| s.trim_end_matches('\r'));

        match self {
            #[cfg(windows)]
            Self::Cmd => {
                let mut buf = String::from("@echo off\r\n");
                for line in lines {
                    buf.push_str(line);
                    buf.push_str("\r\n");
                }
                buf
            }
            #[cfg(windows)]
            Self::PowerShell => {
                let mut buf = String::from(concat!(
                    "$ErrorActionPreference = 'Stop'\r\n",
                    "Set-StrictMode -Version Latest\r\n",
                    "$LASTEXITCODE = 0\r\n\r\n"
                ));

                for line in lines {
                    buf.push_str(line);
                    buf.push_str("\r\n");
                }

                buf.push_str("\r\nexit $LASTEXITCODE\r\n");
                buf
            }
            Self::Pwsh => {
                let mut buf = String::from(concat!(
                    "$ErrorActionPreference = 'Stop'\n",
                    "Set-StrictMode -Version Latest\n",
                    "$PSNativeCommandUseErrorActionPreference = $true\n",
                    "$LASTEXITCODE = 0\n\n"
                ));

                for line in lines {
                    buf.push_str(line);
                    buf.push('\n');
                }

                buf.push_str("\nexit $LASTEXITCODE\n");
                buf
            }
            Self::Bash | Self::Zsh | Self::Sh | Self::Fish | Self::Csh | Self::Nu => {
                let mut buf = String::new();
                for line in lines {
                    buf.push_str(line);
                    buf.push('\n');
                }
                buf
            }
        }
    }

    fn write<W: Write>(self, mut writer: W, script: &str) -> std::io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Cmd => {
                let bytes = encode_oemcp(script)?;
                writer.write_all(&bytes)?;
            }
            #[cfg(windows)]
            Self::PowerShell => {
                writer.write_all(b"\xEF\xBB\xBF")?;
                writer.write_all(script.as_bytes())?;
            }
            Self::Pwsh | Self::Bash | Self::Zsh | Self::Sh | Self::Fish | Self::Csh | Self::Nu => {
                writer.write_all(script.as_bytes())?;
            }
        }
        writer.flush()
    }
}

#[cfg(windows)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::upper_case_acronyms,
    non_camel_case_types
)]
fn encode_oemcp(text: &str) -> std::io::Result<Vec<u8>> {
    type BOOL = i32;
    type DWORD = u32;
    type UINT = u32;
    type LPCWSTR = *const u16;
    type LPSTR = *mut i8;
    type LPCCH = *const i8;
    type LPBOOL = *mut BOOL;

    const CP_OEMCP: UINT = 1;

    unsafe extern "system" {
        fn WideCharToMultiByte(
            CodePage: UINT,
            dwFlags: DWORD,
            lpWideCharStr: LPCWSTR,
            cchWideChar: i32,
            lpMultiByteStr: LPSTR,
            cbMultiByte: i32,
            lpDefaultChar: LPCCH,
            lpUsedDefaultChar: LPBOOL,
        ) -> i32;
    }

    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Ok(Vec::new());
    }

    unsafe {
        let len = WideCharToMultiByte(
            CP_OEMCP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        );

        if len <= 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut bytes = vec![0u8; len as usize];

        let written = WideCharToMultiByte(
            CP_OEMCP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            bytes.as_mut_ptr().cast(),
            len,
            std::ptr::null(),
            std::ptr::null_mut(),
        );

        if written <= 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::Runner;

    #[test]
    fn resolves_and_renders_shells() {
        for (name, suffix, args, expected) in [
            (
                "pwsh",
                ".ps1",
                &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"] as &[&str],
                "$ErrorActionPreference = 'Stop'\n\
                 Set-StrictMode -Version Latest\n\
                 $PSNativeCommandUseErrorActionPreference = $true\n\
                 $LASTEXITCODE = 0\n\n\
                 one\n\
                 two\n\n\
                 exit $LASTEXITCODE\n",
            ),
            (
                "bash",
                ".sh",
                &["--noprofile", "--norc", "-euo", "pipefail"],
                "one\ntwo\n",
            ),
            ("zsh", ".sh", &["-f", "-euo", "pipefail"], "one\ntwo\n"),
            ("fish", ".fish", &["--no-config"], "one\ntwo\n"),
            ("csh", ".csh", &["-f", "-e"], "one\ntwo\n"),
            ("tcsh", ".csh", &["-f", "-e"], "one\ntwo\n"),
            ("nu", ".nu", &["--no-config-file"], "one\ntwo\n"),
            ("sh", ".sh", &["-eu"], "one\ntwo\n"),
            ("dash", ".sh", &["-eu"], "one\ntwo\n"),
            ("ash", ".sh", &["-eu"], "one\ntwo\n"),
            ("ksh", ".sh", &["-eu"], "one\ntwo\n"),
        ] {
            let runner = Runner::from_name(name).unwrap();
            assert_eq!(runner.suffix(), suffix);
            assert_eq!(runner.args(), args);
            assert_eq!(runner.render("one\r\ntwo\n"), expected);
        }

        assert!(Runner::from_name("unknown").is_none());

        #[cfg(windows)]
        {
            let runner = Runner::from_name("cmd").unwrap();
            assert_eq!(runner.suffix(), ".bat");
            assert_eq!(runner.args(), &["/d", "/e:on", "/v:off", "/s", "/c"]);
            assert_eq!(runner.render("one\ntwo"), "@echo off\r\none\r\ntwo\r\n");

            let runner = Runner::from_name("powershell").unwrap();
            assert_eq!(runner.suffix(), ".ps1");
            assert!(runner.render("one").starts_with("$ErrorActionPreference = 'Stop'\r\n"));
        }
    }
}
