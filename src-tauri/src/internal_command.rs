//! Tauri 初始化前执行的内部维护命令。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::payload::{garbage_collect_runtimes, provision_payload};
use crate::runtime::default_runtime_root;

/// 已通过严格参数校验的桌面重启 helper 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RelaunchCommand {
    old_pid: u32,
}

/// 已通过参数边界校验的 provision 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionCommand {
    pub resources: PathBuf,
    pub runtime_root: PathBuf,
}

/// 已通过参数边界校验的安装器 smoke runtime 清理命令。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanupTestRuntimeCommand {
    runtime_root: PathBuf,
}

/// 在 Tauri 和 single-instance 初始化前识别并执行内部命令。
pub fn run_if_requested() -> Option<i32> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| argument == "--relaunch-after-pid")
    {
        let command = match parse_relaunch_arguments(arguments) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("invalid relaunch command: {error}");
                return Some(2);
            }
        };
        return match wait_and_relaunch(command.old_pid) {
            Ok(()) => Some(0),
            Err(error) => {
                eprintln!("desktop relaunch failed: {error}");
                Some(1)
            }
        };
    }
    if arguments
        .iter()
        .any(|argument| argument == "--cleanup-provision-test-runtime")
    {
        let command = match parse_cleanup_arguments(arguments) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("invalid provision test cleanup command: {error}");
                return Some(2);
            }
        };
        return match cleanup_test_runtime_root(&command.runtime_root, &std::env::temp_dir()) {
            Ok(()) => Some(0),
            Err(error) => {
                eprintln!("provision test runtime cleanup failed: {error}");
                Some(1)
            }
        };
    }
    if !arguments
        .iter()
        .any(|argument| argument == "--provision-runtime")
    {
        return None;
    }
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("could not resolve desktop executable for provision: {error}");
            return Some(2);
        }
    };
    let runtime_root = match default_runtime_root() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return Some(2);
        }
    };
    let command = match parse_provision_arguments(arguments, &executable, &runtime_root) {
        Ok(Some(command)) => command,
        Ok(None) => return None,
        Err(error) => {
            eprintln!("invalid provision command: {error}");
            return Some(2);
        }
    };
    match provision_payload(&command.resources, &command.runtime_root, &[1]) {
        Ok(result) => {
            if let Err(error) = garbage_collect_runtimes(&command.runtime_root) {
                eprintln!("runtime was provisioned but garbage collection failed: {error}");
                return Some(1);
            }
            println!("provisioned payload {}", result.payload_digest);
            Some(0)
        }
        Err(error) => {
            eprintln!("runtime provision failed: {error}");
            Some(1)
        }
    }
}

/// 只接受 `--relaunch-after-pid <正整数>`，拒绝混入任何路径或额外参数。
fn parse_relaunch_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<RelaunchCommand, String> {
    let arguments = arguments.into_iter().skip(1).collect::<Vec<_>>();
    if arguments.len() != 2 || arguments[0] != "--relaunch-after-pid" {
        return Err("expected exactly --relaunch-after-pid <pid>".to_owned());
    }
    let old_pid = arguments[1]
        .to_string_lossy()
        .parse::<u32>()
        .map_err(|_| "PID must be a positive 32-bit integer".to_owned())?;
    if old_pid == 0 || old_pid == std::process::id() {
        return Err("PID must identify a different process".to_owned());
    }
    Ok(RelaunchCommand { old_pid })
}

/// 等待旧桌面 PID 退出，再以相同可执行文件启动一个不带用户参数的新实例。
#[cfg(windows)]
fn wait_and_relaunch(old_pid: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE};

    let process = unsafe { OpenProcess(SYNCHRONIZE, 0, old_pid) };
    if !process.is_null() {
        let wait_result = unsafe { WaitForSingleObject(process, INFINITE) };
        unsafe { CloseHandle(process) };
        if wait_result == WAIT_FAILED {
            return Err(format!(
                "could not wait for old desktop PID {old_pid}: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve desktop executable: {error}"))?;
    Command::new(executable)
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start new desktop process: {error}"))
}

#[cfg(not(windows))]
fn wait_and_relaunch(_old_pid: u32) -> Result<(), String> {
    Err("desktop relaunch helper is only supported on Windows".to_owned())
}

/// 解析测试 runtime 清理参数；清理操作必须显式声明 test mode 和唯一目标。
fn parse_cleanup_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<CleanupTestRuntimeCommand, String> {
    let arguments = arguments.into_iter().skip(1).collect::<Vec<_>>();
    let mut test_mode = false;
    let mut runtime_root = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        match argument.as_ref() {
            "--cleanup-provision-test-runtime" => {}
            "--provision-test-mode" => test_mode = true,
            "--runtime-root" => {
                index += 1;
                let value = arguments.get(index).ok_or_else(|| {
                    "--runtime-root requires a non-empty path argument".to_owned()
                })?;
                if value.is_empty() {
                    return Err("--runtime-root requires a non-empty path argument".to_owned());
                }
                runtime_root = Some(PathBuf::from(value));
            }
            _ => return Err(format!("unsupported cleanup argument: {argument}")),
        }
        index += 1;
    }
    if !test_mode {
        return Err("cleanup requires --provision-test-mode".to_owned());
    }
    Ok(CleanupTestRuntimeCommand {
        runtime_root: runtime_root.ok_or_else(|| "cleanup requires --runtime-root".to_owned())?,
    })
}

/// 只删除系统临时目录下、结构精确匹配安装器 smoke 的 canonical runtime 根。
fn cleanup_test_runtime_root(runtime_root: &Path, temp_root: &Path) -> Result<(), String> {
    let canonical_temp = fs::canonicalize(temp_root)
        .map_err(|error| format!("could not canonicalize system temp directory: {error}"))?;
    let canonical_runtime = fs::canonicalize(runtime_root).map_err(|error| {
        format!(
            "could not canonicalize provision test runtime {}: {error}",
            runtime_root.display()
        )
    })?;
    let relative = canonical_runtime
        .strip_prefix(&canonical_temp)
        .map_err(|_| {
            format!(
                "provision test runtime is outside the system temp directory: {}",
                canonical_runtime.display()
            )
        })?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let valid = components.len() == 4
        && components[0]
            .to_ascii_lowercase()
            .starts_with("dsh-desktop-installer-smoke-")
        && components[1].eq_ignore_ascii_case("localappdata")
        && components[2].eq_ignore_ascii_case("dsh-desktop")
        && components[3].eq_ignore_ascii_case("runtime");
    if !valid {
        return Err(format!(
            "provision test runtime has an unexpected path shape: {}",
            canonical_runtime.display()
        ));
    }
    fs::remove_dir_all(&canonical_runtime).map_err(|error| {
        format!(
            "could not remove provision test runtime {}: {error}",
            canonical_runtime.display()
        )
    })
}

/// 解析 provision 参数；路径覆盖仅供隔离安装器 smoke 显式使用。
fn parse_provision_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    executable: &Path,
    default_runtime_root: &Path,
) -> Result<Option<ProvisionCommand>, String> {
    let arguments = arguments.into_iter().skip(1).collect::<Vec<_>>();
    if !arguments
        .iter()
        .any(|argument| argument == "--provision-runtime")
    {
        return Ok(None);
    }
    let mut test_mode = false;
    let mut resources_override = None;
    let mut runtime_override = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        match argument.as_ref() {
            "--provision-runtime" => {}
            "--provision-test-mode" => test_mode = true,
            "--payload-resources" | "--runtime-root" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| format!("{argument} requires a non-empty path argument"))?;
                if value.is_empty() {
                    return Err(format!("{argument} requires a non-empty path argument"));
                }
                if argument == "--payload-resources" {
                    resources_override = Some(PathBuf::from(value));
                } else {
                    runtime_override = Some(PathBuf::from(value));
                }
            }
            _ => return Err(format!("unsupported provision argument: {argument}")),
        }
        index += 1;
    }
    if !test_mode && (resources_override.is_some() || runtime_override.is_some()) {
        return Err("path overrides require --provision-test-mode".to_owned());
    }
    let resources = resources_override.unwrap_or_else(|| {
        executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_owned()
    });
    Ok(Some(ProvisionCommand {
        resources,
        runtime_root: runtime_override.unwrap_or_else(|| default_runtime_root.to_owned()),
    }))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        cleanup_test_runtime_root, parse_cleanup_arguments, parse_provision_arguments,
        parse_relaunch_arguments,
    };

    #[test]
    fn relaunch_requires_exact_pid_only_arguments() {
        let parsed = parse_relaunch_arguments([
            OsString::from("dsh-desktop.exe"),
            OsString::from("--relaunch-after-pid"),
            OsString::from("4242"),
        ])
        .unwrap();
        assert_eq!(parsed.old_pid, 4242);

        assert!(parse_relaunch_arguments([
            OsString::from("dsh-desktop.exe"),
            OsString::from("--relaunch-after-pid"),
            OsString::from("4242"),
            OsString::from("--payload-resources"),
        ])
        .is_err());
        assert!(parse_relaunch_arguments([
            OsString::from("dsh-desktop.exe"),
            OsString::from("--relaunch-after-pid"),
            OsString::from("0"),
        ])
        .is_err());
    }

    #[test]
    fn normal_launch_is_not_intercepted() {
        let parsed = parse_provision_arguments(
            [OsString::from("dsh-desktop.exe")],
            Path::new(r"C:\app\dsh-desktop.exe"),
            Path::new(r"C:\runtime"),
        )
        .unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn provision_defaults_to_executable_directory_and_managed_root() {
        let parsed = parse_provision_arguments(
            [
                OsString::from("dsh-desktop.exe"),
                OsString::from("--provision-runtime"),
            ],
            Path::new(r"C:\app\dsh-desktop.exe"),
            Path::new(r"C:\runtime"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.resources, Path::new(r"C:\app"));
        assert_eq!(parsed.runtime_root, Path::new(r"C:\runtime"));
    }

    #[test]
    fn path_overrides_require_explicit_test_mode() {
        let error = parse_provision_arguments(
            [
                OsString::from("dsh-desktop.exe"),
                OsString::from("--provision-runtime"),
                OsString::from("--runtime-root"),
                OsString::from(r"C:\isolated"),
            ],
            Path::new(r"C:\app\dsh-desktop.exe"),
            Path::new(r"C:\runtime"),
        )
        .unwrap_err();
        assert!(error.contains("provision-test-mode"));

        let parsed = parse_provision_arguments(
            [
                OsString::from("dsh-desktop.exe"),
                OsString::from("--provision-runtime"),
                OsString::from("--provision-test-mode"),
                OsString::from("--runtime-root"),
                OsString::from(r"C:\isolated"),
                OsString::from("--payload-resources"),
                OsString::from(r"C:\payload"),
            ],
            Path::new(r"C:\app\dsh-desktop.exe"),
            Path::new(r"C:\runtime"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.resources, Path::new(r"C:\payload"));
        assert_eq!(parsed.runtime_root, Path::new(r"C:\isolated"));
    }

    #[test]
    fn cleanup_requires_explicit_test_mode_and_runtime_root() {
        let error = parse_cleanup_arguments([
            OsString::from("dsh-desktop.exe"),
            OsString::from("--cleanup-provision-test-runtime"),
            OsString::from("--runtime-root"),
            OsString::from(r"C:\isolated"),
        ])
        .unwrap_err();
        assert!(error.contains("provision-test-mode"));
    }

    #[test]
    fn cleanup_only_accepts_the_exact_installer_smoke_shape() {
        let temp = std::env::temp_dir();
        let smoke = temp.join(format!(
            "dsh-desktop-installer-smoke-test-{}",
            std::process::id()
        ));
        let runtime = smoke.join("localappdata/dsh-desktop/runtime");
        fs::create_dir_all(&runtime).expect("应创建测试 runtime");
        fs::write(runtime.join("sentinel"), b"test").expect("应写入测试文件");
        cleanup_test_runtime_root(&runtime, &temp).expect("合法 smoke runtime 应删除");
        assert!(!runtime.exists());

        let unsafe_root: PathBuf = smoke.join("localappdata/dsh-desktop/not-runtime");
        fs::create_dir_all(&unsafe_root).expect("应创建错误结构目录");
        let error = cleanup_test_runtime_root(&unsafe_root, &temp).expect_err("错误结构必须拒绝");
        assert!(error.contains("unexpected path shape"));
        let _ = fs::remove_dir_all(smoke);
    }
}
