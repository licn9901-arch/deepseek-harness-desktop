//! 不依赖 DSH Host 的桌面壳插件安全管理与故障恢复。

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::{Mutex, RwLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::host::HostSupervisor;
use crate::lifecycle::HostController;
use crate::logger::{log_app, log_error, log_file_path};
use crate::plugins::PluginLock;
use crate::runtime::RuntimePaths;

const OVERRIDE_FILE: &str = "desktop-managed/plugin-recovery.json";
const MARKET_STATE_FILE: &str = "profiles/web/.dsh-market/state.json";
const PROFILE_FILE: &str = "profiles/web/package.json";
const MANAGER_WINDOW: &str = "plugin-manager";

/// 壳层永远不能禁用或卸载的核心 bundle。
pub const PROTECTED_BUNDLES: [&str; 4] = [
    "@deepseek-ai/dsh-base",
    "@deepseek-ai/dsh-web-app",
    "dshmarket",
    "@dsh-desktop/runtime-services",
];

/// 单个插件被壳层禁用前需要保留的恢复信息。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DisabledPlugin {
    pub previous_index: usize,
    #[serde(default)]
    pub patch_rows: Vec<String>,
}

/// 跨桌面升级持久化的插件禁用状态。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryOverrideState {
    pub schema_version: u32,
    #[serde(default)]
    pub disabled: BTreeMap<String, DisabledPlugin>,
}

/// 壳层页面展示的单个插件记录。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub package: String,
    pub label: String,
    pub version: String,
    pub source: String,
    pub enabled: bool,
    pub installed: bool,
    pub protected: bool,
    pub can_uninstall: bool,
    pub issue: Option<String>,
}

/// 壳层页面一次读取所需的完整状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySnapshot {
    pub plugins: Vec<PluginRecord>,
    pub failure: Option<String>,
    pub log_path: String,
    pub restart_required: bool,
}

/// 保存当前运行时、最近一次失败和插件写操作串行锁。
pub struct PluginRecoveryState {
    resource_dir: PathBuf,
    runtime: RwLock<Option<RuntimePaths>>,
    failure: RwLock<Option<String>>,
    restart_required: RwLock<bool>,
    operation: Mutex<()>,
}

impl PluginRecoveryState {
    /// 创建尚未完成 runtime 解析的恢复控制器。
    pub fn new(resource_dir: PathBuf) -> Self {
        Self {
            resource_dir,
            runtime: RwLock::new(None),
            failure: RwLock::new(None),
            restart_required: RwLock::new(false),
            operation: Mutex::new(()),
        }
    }

    /// 更新本次启动实际选择的 runtime，供壳层直接访问对应插件锁和 CLI。
    pub fn set_runtime(&self, runtime: RuntimePaths) {
        *self
            .runtime
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(runtime);
    }

    /// 记录经过长度限制的失败摘要，避免把无限 Host 输出送入页面。
    pub fn record_failure(&self, message: &str) {
        let summary = message.chars().take(4000).collect::<String>();
        *self
            .failure
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(summary);
    }

    fn runtime(&self) -> Result<RuntimePaths, String> {
        if let Some(runtime) = self
            .runtime
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            return Ok(runtime);
        }
        RuntimePaths::resolve(&self.resource_dir)
    }
}

impl Default for RecoveryOverrideState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            disabled: BTreeMap::new(),
        }
    }
}

/// 返回包名是否可以作为无 shell 子进程的单一 argv 参数。
pub fn is_valid_package_name(package: &str) -> bool {
    static PACKAGE_NAME: OnceLock<Regex> = OnceLock::new();
    if package.len() > 214 {
        return false;
    }
    PACKAGE_NAME
        .get_or_init(|| {
            Regex::new(
                r"^(?:@[a-z0-9](?:[a-z0-9._~-]*[a-z0-9])?/[a-z0-9](?:[a-z0-9._~-]*[a-z0-9])?|[a-z0-9](?:[a-z0-9._~-]*[a-z0-9])?)$",
            )
            .expect("编译期 npm 包名正则必须有效")
        })
        .is_match(package)
}

/// 在最终 profile 上应用壳层禁用覆盖，保证 Host 启动前故障插件不进入 bundle 栈。
pub fn apply_disabled_overrides(
    profile: &mut Value,
    state: &RecoveryOverrideState,
) -> Result<bool, String> {
    if state.schema_version != 1 {
        return Err(format!(
            "unsupported plugin recovery state schema: {}",
            state.schema_version
        ));
    }
    let bundles = profile
        .get_mut("dsh")
        .and_then(Value::as_object_mut)
        .and_then(|dsh| dsh.get_mut("profile"))
        .and_then(Value::as_object_mut)
        .and_then(|profile| profile.get_mut("bundles"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "profile field \"dsh.profile.bundles\" must be an array".to_owned())?;
    let before = bundles.len();
    bundles.retain(|bundle| {
        let Some(package) = bundle.as_str() else {
            return true;
        };
        PROTECTED_BUNDLES.contains(&package) || !state.disabled.contains_key(package)
    });
    Ok(bundles.len() != before)
}

/// 从 DSH home 读取恢复覆盖；缺失文件表示没有壳层禁用项。
pub fn read_override_state(dsh_home: &Path) -> Result<RecoveryOverrideState, String> {
    let path = dsh_home.join(OVERRIDE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecoveryOverrideState::default())
        }
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let state: RecoveryOverrideState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid plugin recovery state {}: {error}", path.display()))?;
    if state.schema_version != 1 {
        return Err(format!(
            "unsupported plugin recovery state schema: {}",
            state.schema_version
        ));
    }
    for package in state.disabled.keys() {
        if !is_valid_package_name(package) {
            return Err(format!(
                "invalid package in plugin recovery state: {package}"
            ));
        }
    }
    Ok(state)
}

/// 使用同目录临时文件原子替换内容，避免应用中断留下半个 JSON。
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plugin-recovery"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    replace_file(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to activate {}: {error}", path.display())
    })
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode {}: {error}", path.display()))?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

fn read_json_value(path: &Path) -> Result<Value, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid JSON {}: {error}", path.display()))
}

fn profile_bundles(profile: &Value) -> Result<Vec<String>, String> {
    profile
        .get("dsh")
        .and_then(|value| value.get("profile"))
        .and_then(|value| value.get("bundles"))
        .and_then(Value::as_array)
        .ok_or_else(|| "profile field \"dsh.profile.bundles\" must be an array".to_owned())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "profile bundle must be a string".to_owned())
        })
        .collect()
}

fn profile_dependencies(profile: &Value) -> Result<BTreeMap<String, String>, String> {
    let object = profile
        .get("dependencies")
        .and_then(Value::as_object)
        .ok_or_else(|| "profile field \"dependencies\" must be an object".to_owned())?;
    object
        .iter()
        .map(|(package, spec)| {
            let spec = spec
                .as_str()
                .ok_or_else(|| format!("dependency {package} must be a string"))?;
            Ok((package.clone(), spec.to_owned()))
        })
        .collect()
}

fn package_version(modules: &Path, package: &str) -> Option<String> {
    let relative = package.trim_start_matches('@').replace('/', "\\");
    let path = if package.starts_with('@') {
        let (scope, name) = package.split_once('/')?;
        modules.join(scope).join(name).join("package.json")
    } else {
        modules.join(relative).join("package.json")
    };
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value.get("version")?.as_str().map(str::to_owned)
}

fn friendly_label(package: &str) -> String {
    match package {
        "@deepseek-ai/dsh-base" => "DSH Core".to_owned(),
        "@deepseek-ai/dsh-web-app" => "DSH Web App".to_owned(),
        "dshmarket" => "DSH Market".to_owned(),
        "@dsh-desktop/runtime-services" => "Desktop Runtime Services".to_owned(),
        "@dsh-desktop/settings" => "Desktop Settings".to_owned(),
        "@changfenhuang/dsh-genui" => "GenUI".to_owned(),
        "dsh-better-sidebar" => "Better Sidebar".to_owned(),
        "@linxin666/dsh-client-ui-skin-center" => "Skin Center".to_owned(),
        "@vectorize-io/hindsight-coding-agents" => "Hindsight".to_owned(),
        "@cubee-slide/skills-mcp-manager" => "Skills / MCP Manager".to_owned(),
        _ => package.to_owned(),
    }
}

fn read_market_disabled(dsh_home: &Path) -> Result<(Value, BTreeSet<String>), String> {
    let path = dsh_home.join(MARKET_STATE_FILE);
    let mut value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("invalid Market state {}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({"disabled": [], "groups": {}, "groupOrder": []})
        }
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    if !value.is_object() {
        return Err(format!("Market state {} must be an object", path.display()));
    }
    let disabled = value
        .get("disabled")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if value.get("disabled").is_none() {
        value["disabled"] = Value::Array(Vec::new());
    }
    Ok((value, disabled))
}

fn write_market_disabled(
    dsh_home: &Path,
    mut value: Value,
    disabled: &BTreeSet<String>,
) -> Result<(), String> {
    value["disabled"] = Value::Array(disabled.iter().cloned().map(Value::String).collect());
    write_json(&dsh_home.join(MARKET_STATE_FILE), &value)
}

/// 从锁文件、profile 和实际安装目录组合出壳层可管理列表。
pub fn discover_plugins(runtime: &RuntimePaths) -> Result<Vec<PluginRecord>, String> {
    let profile_path = runtime.dsh_home.join(PROFILE_FILE);
    let profile = read_json_value(&profile_path)?;
    let dependencies = profile_dependencies(&profile)?;
    let bundles = profile_bundles(&profile)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let overrides = read_override_state(&runtime.dsh_home)?;
    let (_, market_disabled) = read_market_disabled(&runtime.dsh_home)?;
    let lock_path = runtime.plugins_root.join("plugins.lock.json");
    let lock = PluginLock::parse(
        &fs::read(&lock_path)
            .map_err(|error| format!("failed to read {}: {error}", lock_path.display()))?,
    )?;
    let managed = lock
        .plugins
        .iter()
        .map(|plugin| plugin.package.clone())
        .collect::<BTreeSet<_>>();
    let mut records = Vec::new();

    for package in PROTECTED_BUNDLES {
        let version = lock
            .plugins
            .iter()
            .find(|plugin| plugin.package == package)
            .map(|plugin| plugin.version.clone())
            .or_else(|| package_version(&runtime.host_root.join("node_modules"), package))
            .unwrap_or_else(|| "内置".to_owned());
        records.push(PluginRecord {
            package: package.to_owned(),
            label: friendly_label(package),
            version,
            source: "system".to_owned(),
            enabled: true,
            installed: true,
            protected: true,
            can_uninstall: false,
            issue: None,
        });
    }

    for plugin in lock
        .plugins
        .iter()
        .filter(|plugin| !PROTECTED_BUNDLES.contains(&plugin.package.as_str()))
    {
        let installed = dependencies.contains_key(&plugin.package);
        records.push(PluginRecord {
            package: plugin.package.clone(),
            label: friendly_label(&plugin.package),
            version: plugin.version.clone(),
            source: "builtin".to_owned(),
            enabled: bundles.contains(&plugin.package)
                && !overrides.disabled.contains_key(&plugin.package)
                && !market_disabled.contains(&plugin.package),
            installed,
            protected: false,
            can_uninstall: false,
            issue: (!installed).then(|| "内置插件链接缺失".to_owned()),
        });
    }

    for (package, spec) in dependencies.iter().filter(|(package, _)| {
        !managed.contains(*package) && !PROTECTED_BUNDLES.contains(&package.as_str())
    }) {
        let installed_version = package_version(&runtime.web_profile.join("node_modules"), package);
        let installed = installed_version.is_some();
        records.push(PluginRecord {
            package: package.clone(),
            label: friendly_label(package),
            version: installed_version.unwrap_or_else(|| spec.clone()),
            source: "user".to_owned(),
            enabled: bundles.contains(package)
                && !overrides.disabled.contains_key(package)
                && !market_disabled.contains(package),
            installed,
            protected: false,
            can_uninstall: true,
            issue: (!installed).then(|| "依赖存在，但插件文件缺失".to_owned()),
        });
    }

    let known = records
        .iter()
        .map(|record| record.package.clone())
        .collect::<BTreeSet<_>>();
    for package in bundles.iter().filter(|package| !known.contains(*package)) {
        records.push(PluginRecord {
            package: package.clone(),
            label: friendly_label(package),
            version: "未知".to_owned(),
            source: "user".to_owned(),
            enabled: !overrides.disabled.contains_key(package),
            installed: false,
            protected: false,
            can_uninstall: false,
            issue: Some("bundle 已启用，但 profile 中没有对应依赖".to_owned()),
        });
    }
    Ok(records)
}

fn set_profile_bundles(profile: &mut Value, bundles: Vec<String>) -> Result<(), String> {
    let target = profile
        .get_mut("dsh")
        .and_then(Value::as_object_mut)
        .and_then(|dsh| dsh.get_mut("profile"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "profile field \"dsh.profile\" must be an object".to_owned())?;
    target.insert(
        "bundles".to_owned(),
        Value::Array(bundles.into_iter().map(Value::String).collect()),
    );
    Ok(())
}

fn builtin_restore_index(
    runtime: &RuntimePaths,
    package: &str,
    bundles: &[String],
) -> Result<Option<usize>, String> {
    let lock_path = runtime.plugins_root.join("plugins.lock.json");
    let lock = PluginLock::parse(
        &fs::read(&lock_path)
            .map_err(|error| format!("failed to read {}: {error}", lock_path.display()))?,
    )?;
    let ordered = lock
        .plugins
        .iter()
        .map(|plugin| plugin.package.as_str())
        .collect::<Vec<_>>();
    let Some(target) = ordered.iter().position(|candidate| *candidate == package) else {
        return Ok(None);
    };
    for predecessor in ordered[..target].iter().rev() {
        if let Some(index) = bundles.iter().position(|bundle| bundle == predecessor) {
            return Ok(Some(index + 1));
        }
    }
    for successor in &ordered[target + 1..] {
        if let Some(index) = bundles.iter().position(|bundle| bundle == successor) {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn patch_marker(package: &str, row_id: &str) -> String {
    format!(
        "\n# dsh-desktop-recovery package={package} id={row_id}\n- id: '{row_id}'\n  disabled: true\n"
    )
}

fn patch_row_ids(content: &str, package: &str) -> Result<Vec<String>, String> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|error| format!("invalid cordis.patch.yml: {error}"))?;
    let entries = yaml
        .as_sequence()
        .ok_or_else(|| "cordis.patch.yml root must be a sequence".to_owned())?;
    let valid_id = Regex::new(r"^[A-Za-z0-9._:@/-]{1,128}$").expect("编译期 patch id 正则必须有效");
    let mut rows = Vec::new();
    for entry in entries {
        let Some(insertions) = entry
            .as_mapping()
            .and_then(|mapping| mapping.get(serde_yaml::Value::String("insert".to_owned())))
            .and_then(serde_yaml::Value::as_sequence)
        else {
            continue;
        };
        for insertion in insertions {
            let Some(mapping) = insertion.as_mapping() else {
                continue;
            };
            let name = mapping
                .get(serde_yaml::Value::String("name".to_owned()))
                .and_then(serde_yaml::Value::as_str);
            let id = mapping
                .get(serde_yaml::Value::String("id".to_owned()))
                .and_then(serde_yaml::Value::as_str);
            if name == Some(package) && id.is_some_and(|value| valid_id.is_match(value)) {
                rows.push(id.expect("前置条件已确认 id").to_owned());
            }
        }
    }
    rows.sort();
    rows.dedup();
    Ok(rows)
}

fn prepare_patch_disable(
    path: &Path,
    package: &str,
) -> Result<(Option<String>, Vec<String>), String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((None, Vec::new()))
        }
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let rows = patch_row_ids(&content, package)?;
    let mut next = content.clone();
    for row in &rows {
        let marker = patch_marker(package, row);
        if !next.contains(&marker) {
            next.push_str(&marker);
        }
    }
    Ok(((next != content).then_some(next), rows))
}

fn remove_owned_patch_rows(path: &Path, package: &str, rows: &[String]) -> Result<(), String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let mut next = content.clone();
    for row in rows {
        next = next.replace(&patch_marker(package, row), "");
    }
    if next != content {
        atomic_write(path, next.as_bytes())?;
    }
    Ok(())
}

fn set_plugin_enabled(runtime: &RuntimePaths, package: &str, enabled: bool) -> Result<(), String> {
    if !is_valid_package_name(package) {
        return Err("invalid plugin package name".to_owned());
    }
    let records = discover_plugins(runtime)?;
    let record = records
        .iter()
        .find(|record| record.package == package)
        .ok_or_else(|| "plugin is not part of the current profile".to_owned())?;
    if record.protected {
        return Err("protected plugin cannot be changed".to_owned());
    }
    if enabled && !record.installed {
        return Err("plugin files are missing; reinstall before enabling".to_owned());
    }

    let profile_path = runtime.dsh_home.join(PROFILE_FILE);
    let override_path = runtime.dsh_home.join(OVERRIDE_FILE);
    let mut profile = read_json_value(&profile_path)?;
    let mut bundles = profile_bundles(&profile)?;
    let mut overrides = read_override_state(&runtime.dsh_home)?;
    let (market, mut market_disabled) = read_market_disabled(&runtime.dsh_home)?;
    let patch_path = runtime.dsh_home.join("cordis.patch.yml");

    if enabled {
        let previous = overrides.disabled.get(package).cloned();
        if !bundles.iter().any(|bundle| bundle == package) {
            let index = if record.source == "builtin" {
                builtin_restore_index(runtime, package, &bundles)?
            } else {
                None
            }
            .or_else(|| {
                previous
                    .as_ref()
                    .map(|item| item.previous_index.min(bundles.len()))
            })
            .unwrap_or(bundles.len());
            bundles.insert(index, package.to_owned());
        }
        if let Some(previous) = &previous {
            remove_owned_patch_rows(&patch_path, package, &previous.patch_rows)?;
        }
        market_disabled.remove(package);
        set_profile_bundles(&mut profile, bundles)?;
        // 启用时最后清除覆盖；中途退出仍保持禁用，不会把故障插件带回启动链路。
        write_json(&profile_path, &profile)?;
        write_market_disabled(&runtime.dsh_home, market, &market_disabled)?;
        overrides.disabled.remove(package);
        write_json(&override_path, &overrides)?;
    } else {
        let (patch_content, patch_rows) = prepare_patch_disable(&patch_path, package)?;
        let previous_index = bundles
            .iter()
            .position(|bundle| bundle == package)
            .unwrap_or(bundles.len());
        overrides
            .disabled
            .entry(package.to_owned())
            .and_modify(|item| item.patch_rows = patch_rows.clone())
            .or_insert(DisabledPlugin {
                previous_index,
                patch_rows,
            });
        // 禁用时先落覆盖；后续任何写入失败，下一次 prepare 仍会过滤目标 bundle。
        write_json(&override_path, &overrides)?;
        if let Some(content) = patch_content {
            atomic_write(&patch_path, content.as_bytes())?;
        }
        bundles.retain(|bundle| bundle != package);
        market_disabled.insert(package.to_owned());
        set_profile_bundles(&mut profile, bundles)?;
        write_json(&profile_path, &profile)?;
        write_market_disabled(&runtime.dsh_home, market, &market_disabled)?;
    }
    Ok(())
}

fn build_plugin_path(runtime: &RuntimePaths) -> Result<OsString, String> {
    let node_directory = runtime
        .node
        .parent()
        .ok_or_else(|| "bundled Node has no parent directory".to_owned())?;
    let mut paths = vec![
        node_directory.to_owned(),
        runtime.tool_bin_directory.clone(),
    ];
    if let Some(inherited) = env::var_os("PATH") {
        paths.extend(env::split_paths(&inherited));
    }
    env::join_paths(paths).map_err(|error| format!("failed to construct plugin PATH: {error}"))
}

fn uninstall_can_reconcile(command_succeeded: bool, package_present: bool) -> bool {
    command_succeeded || !package_present
}

fn uninstall_user_plugin(runtime: &RuntimePaths, package: &str) -> Result<(), String> {
    let record = discover_plugins(runtime)?
        .into_iter()
        .find(|record| record.package == package)
        .ok_or_else(|| "plugin is not part of the current profile".to_owned())?;
    if !record.can_uninstall || record.source != "user" {
        return Err("only user-installed plugins can be uninstalled".to_owned());
    }
    set_plugin_enabled(runtime, package, false)?;
    let path = build_plugin_path(runtime)?;
    let mut command = Command::new(&runtime.node);
    command
        .arg(&runtime.cli_entry)
        .args(["plugin", "--profile", "web", "remove", package])
        .current_dir(&runtime.web_profile)
        .env("DSH_HOME", &runtime.dsh_home)
        .env("PATH", path)
        .env("npm_config_manage_package_manager_versions", "false")
        .env("COREPACK_ENABLE_PROJECT_SPEC", "0")
        .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to start DSH plugin removal: {error}"))?;

    let profile_path = runtime.dsh_home.join(PROFILE_FILE);
    let mut profile = read_json_value(&profile_path)?;
    let package_present =
        package_version(&runtime.web_profile.join("node_modules"), package).is_some();
    if !uninstall_can_reconcile(output.status.success(), package_present) {
        let detail = String::from_utf8_lossy(&output.stderr)
            .chars()
            .rev()
            .take(1200)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        return Err(format!("插件卸载失败，已保持禁用。{detail}"));
    }

    if let Some(dependencies) = profile
        .get_mut("dependencies")
        .and_then(Value::as_object_mut)
    {
        dependencies.remove(package);
    }
    let mut bundles = profile_bundles(&profile)?;
    bundles.retain(|bundle| bundle != package);
    set_profile_bundles(&mut profile, bundles)?;
    write_json(&profile_path, &profile)?;

    let override_path = runtime.dsh_home.join(OVERRIDE_FILE);
    let mut overrides = read_override_state(&runtime.dsh_home)?;
    if let Some(previous) = overrides.disabled.get(package) {
        remove_owned_patch_rows(
            &runtime.dsh_home.join("cordis.patch.yml"),
            package,
            &previous.patch_rows,
        )?;
    }
    overrides.disabled.remove(package);
    write_json(&override_path, &overrides)?;
    let (mut market, mut disabled) = read_market_disabled(&runtime.dsh_home)?;
    disabled.remove(package);
    if let Some(groups) = market.get_mut("groups").and_then(Value::as_object_mut) {
        for members in groups.values_mut().filter_map(Value::as_array_mut) {
            members.retain(|member| member.as_str() != Some(package));
        }
    }
    write_market_disabled(&runtime.dsh_home, market, &disabled)?;
    Ok(())
}

fn assert_manager_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != MANAGER_WINDOW {
        return Err("plugin recovery commands are limited to the local manager window".to_owned());
    }
    Ok(())
}

fn is_local_manager_url(url: &url::Url) -> bool {
    url.scheme() == "tauri" || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
}

fn stop_host_for_mutation(app: &AppHandle) {
    if let Some(controller) = app.try_state::<HostController>() {
        controller.mark_failed();
    }
    if let Some(supervisor) = app.try_state::<HostSupervisor>() {
        supervisor.shutdown_for_recovery();
    }
}

/// 返回当前插件与最近一次启动失败，供本地恢复页渲染。
#[tauri::command]
pub fn recovery_plugin_list(
    window: WebviewWindow,
    state: tauri::State<'_, PluginRecoveryState>,
) -> Result<RecoverySnapshot, String> {
    assert_manager_window(&window)?;
    let runtime = state.runtime()?;
    Ok(RecoverySnapshot {
        plugins: discover_plugins(&runtime)?,
        failure: state
            .failure
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone(),
        log_path: log_file_path().display().to_string(),
        restart_required: *state
            .restart_required
            .read()
            .unwrap_or_else(|error| error.into_inner()),
    })
}

/// 串行启用或禁用一个已枚举插件；变更前停止 Host，避免并发写 profile。
#[tauri::command]
pub fn recovery_plugin_set_enabled(
    window: WebviewWindow,
    app: AppHandle,
    state: tauri::State<'_, PluginRecoveryState>,
    package: String,
    enabled: bool,
) -> Result<(), String> {
    assert_manager_window(&window)?;
    let _operation = state
        .operation
        .lock()
        .map_err(|_| "plugin recovery operation lock is poisoned".to_owned())?;
    let runtime = state.runtime()?;
    stop_host_for_mutation(&app);
    set_plugin_enabled(&runtime, &package, enabled)?;
    *state
        .restart_required
        .write()
        .unwrap_or_else(|error| error.into_inner()) = true;
    log_app(&format!(
        "plugin recovery changed package={package} enabled={enabled}"
    ));
    Ok(())
}

/// 串行卸载用户插件；内置插件和系统组件始终拒绝。
#[tauri::command]
pub fn recovery_plugin_uninstall(
    window: WebviewWindow,
    app: AppHandle,
    state: tauri::State<'_, PluginRecoveryState>,
    package: String,
) -> Result<(), String> {
    assert_manager_window(&window)?;
    let _operation = state
        .operation
        .lock()
        .map_err(|_| "plugin recovery operation lock is poisoned".to_owned())?;
    let runtime = state.runtime()?;
    stop_host_for_mutation(&app);
    let result = uninstall_user_plugin(&runtime, &package);
    *state
        .restart_required
        .write()
        .unwrap_or_else(|error| error.into_inner()) = true;
    match &result {
        Ok(()) => log_app(&format!("plugin recovery uninstalled package={package}")),
        Err(error) => log_error(&format!(
            "plugin recovery uninstall failed package={package}: {error}"
        )),
    }
    result
}

/// 启动内部等待 helper，并退出当前应用；helper 在旧 PID 结束后重新拉起桌面。
#[tauri::command]
pub fn recovery_relaunch(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    assert_manager_window(&window)?;
    let executable = env::current_exe()
        .map_err(|error| format!("could not resolve desktop executable: {error}"))?;
    let mut helper = Command::new(executable);
    helper.args(["--relaunch-after-pid", &std::process::id().to_string()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        helper.creation_flags(0x0800_0000);
    }
    helper
        .spawn()
        .map_err(|error| format!("failed to start desktop relaunch helper: {error}"))?;
    app.state::<crate::desktop::DesktopLifecycle>()
        .request_quit();
    app.exit(0);
    Ok(())
}

/// 打开或聚焦独立本地插件恢复窗口。
pub fn open_plugin_manager(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MANAGER_WINDOW) {
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }
    WebviewWindowBuilder::new(
        app,
        MANAGER_WINDOW,
        WebviewUrl::App("plugin-manager.html".into()),
    )
    .title("插件管理与安全恢复")
    .inner_size(880.0, 720.0)
    .min_inner_size(720.0, 560.0)
    .on_navigation(is_local_manager_url)
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .build()
    .map(|_| ())
    .map_err(|error| format!("failed to create plugin manager window: {error}"))
}

/// 保存失败摘要并自动打开恢复页；应用进程保持存活等待用户处理。
pub fn show_failure_recovery(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<PluginRecoveryState>() {
        state.record_failure(message);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    if let Err(error) = open_plugin_manager(app) {
        log_error(&error);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        apply_disabled_overrides, is_local_manager_url, is_valid_package_name, patch_marker,
        patch_row_ids, uninstall_can_reconcile, DisabledPlugin, RecoveryOverrideState,
    };

    /// 仅接受 npm 包名，拒绝路径、版本 spec 和 shell 元字符。
    #[test]
    fn package_name_validation_accepts_only_registry_names() {
        for package in ["dsh-context", "@scope/dsh-plugin", "a.b_c~d"] {
            assert!(is_valid_package_name(package), "应接受 {package}");
        }
        for package in [
            "",
            "../plugin",
            "C:\\plugin",
            "dsh-context@latest",
            "github:owner/repo",
            "plugin && whoami",
            "@scope",
        ] {
            assert!(!is_valid_package_name(package), "应拒绝 {package}");
        }
    }

    /// 禁用覆盖只移除目标 bundle，保持未知用户插件顺序。
    #[test]
    fn disabled_override_filters_bundle_without_reordering_users() {
        let mut profile = json!({
            "dsh": { "profile": { "bundles": [
                "@deepseek-ai/dsh-base",
                "user-before",
                "@changfenhuang/dsh-genui",
                "user-after"
            ] } }
        });
        let state = RecoveryOverrideState {
            schema_version: 1,
            disabled: BTreeMap::from([(
                "@changfenhuang/dsh-genui".to_owned(),
                DisabledPlugin {
                    previous_index: 2,
                    patch_rows: Vec::new(),
                },
            )]),
        };

        assert!(apply_disabled_overrides(&mut profile, &state).unwrap());
        assert_eq!(
            profile["dsh"]["profile"]["bundles"],
            json!(["@deepseek-ai/dsh-base", "user-before", "user-after"])
        );
    }

    /// 即使状态文件被篡改，核心 bundle 也不能被禁用。
    #[test]
    fn disabled_override_ignores_protected_bundles() {
        let mut profile = json!({
            "dsh": { "profile": { "bundles": [
                "@deepseek-ai/dsh-base",
                "@deepseek-ai/dsh-web-app",
                "@dsh-desktop/runtime-services",
                "dshmarket"
            ] } }
        });
        let state = RecoveryOverrideState {
            schema_version: 1,
            disabled: [
                "@deepseek-ai/dsh-base",
                "@deepseek-ai/dsh-web-app",
                "@dsh-desktop/runtime-services",
                "dshmarket",
            ]
            .into_iter()
            .map(|package| {
                (
                    package.to_owned(),
                    DisabledPlugin {
                        previous_index: 0,
                        patch_rows: Vec::new(),
                    },
                )
            })
            .collect(),
        };

        assert!(!apply_disabled_overrides(&mut profile, &state).unwrap());
        assert_eq!(
            profile["dsh"]["profile"]["bundles"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    /// 错误 schema 和错误 profile 类型必须阻断启动前写入。
    #[test]
    fn disabled_override_rejects_invalid_state_and_profile() {
        let mut profile = json!({ "dsh": { "profile": { "bundles": "bad" } } });
        let state = RecoveryOverrideState::default();
        assert!(apply_disabled_overrides(&mut profile, &state).is_err());

        let mut valid = json!({ "dsh": { "profile": { "bundles": [] } } });
        let invalid = RecoveryOverrideState {
            schema_version: 99,
            disabled: BTreeMap::new(),
        };
        assert!(apply_disabled_overrides(&mut valid, &invalid).is_err());
    }

    /// 只为用户 patch 中明确插入该包的合法 loader id 生成壳层覆盖。
    #[test]
    fn patch_rows_are_discovered_without_accepting_injected_ids() {
        let patch = r#"
- insert:
    - id: safe-loader
      name: '@scope/plugin'
    - id: "bad\nid"
      name: '@scope/plugin'
- insert:
    - id: another-loader
      name: other-plugin
"#;
        assert_eq!(
            patch_row_ids(patch, "@scope/plugin").unwrap(),
            vec!["safe-loader"]
        );
        assert!(patch_row_ids("not: [valid", "@scope/plugin").is_err());
    }

    /// 壳层补丁块带稳定所有权标记，启用时可精确移除而不触碰用户原文。
    #[test]
    fn owned_patch_marker_is_deterministic_and_yaml_safe() {
        assert_eq!(
            patch_marker("@scope/plugin", "safe-loader"),
            "\n# dsh-desktop-recovery package=@scope/plugin id=safe-loader\n- id: 'safe-loader'\n  disabled: true\n"
        );
    }

    /// 恢复窗口拒绝跳转到远程页面，避免远程内容继承窗口标签后尝试 IPC。
    #[test]
    fn manager_navigation_accepts_only_packaged_origins() {
        for allowed in [
            "tauri://localhost/plugin-manager.html",
            "http://tauri.localhost/plugin-manager.html",
        ] {
            assert!(is_local_manager_url(&url::Url::parse(allowed).unwrap()));
        }
        for denied in [
            "https://example.com/",
            "file:///C:/temp/plugin-manager.html",
            "http://localhost:3000/",
        ] {
            assert!(!is_local_manager_url(&url::Url::parse(denied).unwrap()));
        }
    }

    /// CLI 报错但包目录已删除时继续收敛 profile；包仍在时保持禁用并报告失败。
    #[test]
    fn uninstall_reconciles_only_success_or_already_removed_package() {
        assert!(uninstall_can_reconcile(true, true));
        assert!(uninstall_can_reconcile(false, false));
        assert!(!uninstall_can_reconcile(false, true));
    }
}
