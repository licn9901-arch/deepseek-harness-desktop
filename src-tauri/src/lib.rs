//! DeepSeek Harness Desktop 的桌面封装与生命周期入口。

pub mod desktop;
pub mod host;
pub mod internal_command;
pub mod lifecycle;
pub mod logger;
pub mod navigation;
pub mod payload;
pub mod plugin_recovery;
pub mod plugins;
pub mod readiness;
pub mod runtime;
pub mod sidebar_settings;

use std::io::{BufRead, BufReader};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const CANDIDATE_CORE_READY_TIMEOUT: Duration = Duration::from_secs(180);
const CANDIDATE_PLUGIN_READY_TIMEOUT: Duration = Duration::from_secs(60);

use desktop::{
    configure_close_to_tray, create_tray, open_external_url, quit_application, show_main_window,
};
use host::HostSupervisor;
use lifecycle::{HostCommand, HostController, HostEvent, LifecycleAction, LifecycleStateMachine};
use logger::{log_app, log_error, log_file_path, log_host};
use navigation::{
    decide_navigation, decide_new_window, safe_target_description, NavigationDecision,
};
use payload::{
    promote_candidate, read_runtime_state, reject_candidate, rollback_candidate_promotion,
};
use plugins::{PluginManager, PluginTransaction};
use readiness::{ReadinessParser, ReadinessSignal};
use runtime::{test_webview_data_directory, RuntimePaths};
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

/// 构建并运行桌面应用，负责窗口、Host 和退出清理的顶层编排。
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, arguments, _cwd| {
                if arguments
                    .iter()
                    .any(|argument| argument == "--quit-existing")
                {
                    log_app("secondary launch requested explicit exit");
                    quit_application(app);
                    return;
                }
                log_app("secondary launch requested; focusing existing window");
                show_main_window(app);
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            plugin_recovery::recovery_plugin_list,
            plugin_recovery::recovery_plugin_set_enabled,
            plugin_recovery::recovery_plugin_uninstall,
            plugin_recovery::recovery_relaunch
        ])
        .setup(setup_application)
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                app_handle
                    .state::<desktop::DesktopLifecycle>()
                    .request_quit();
                if let Some(controller) = app_handle.try_state::<HostController>() {
                    controller.mark_stopping();
                }
                if let Some(supervisor) = app_handle.try_state::<HostSupervisor>() {
                    supervisor.shutdown();
                }
            }
        });
}

/// 创建启动页、启动唯一 Host，并注册读取与监视线程。
fn setup_application(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let boot_started = Instant::now();
    let boot_id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    log_boot_phase(&boot_id, "managed", "boot_start", Duration::ZERO);
    let handle = app.handle().clone();
    let window_title = format!("DeepSeek Harness Desktop · v{}", app.package_info().version);
    app.manage(desktop::DesktopLifecycle::default());
    let (host_controller, host_commands) = HostController::new();
    app.manage(host_controller);
    let host_origin = Arc::new(RwLock::new(None::<url::Url>));
    let navigation_origin = host_origin.clone();
    let new_window_origin = host_origin.clone();
    let resource_dir = app.path().resource_dir().unwrap_or_default();
    app.manage(plugin_recovery::PluginRecoveryState::new(
        resource_dir.clone(),
    ));
    let runtime_started = Instant::now();
    let selection = RuntimePaths::resolve_startup(&resource_dir);
    log_boot_phase(
        &boot_id,
        "managed",
        "runtime_resolved",
        runtime_started.elapsed(),
    );
    // 普通 active runtime 的插件校验与 WebView2 初始化互不依赖，可并行执行以缩短启动关键路径。
    let mut background_plugin_prepare = selection.as_ref().ok().and_then(|selection| {
        (!selection
            .primary
            .activation
            .as_ref()
            .is_some_and(|activation| activation.candidate))
        .then(|| {
            let manager = PluginManager::new(&selection.primary);
            std::thread::spawn(move || {
                let started = Instant::now();
                (manager.prepare(), started.elapsed())
            })
        })
    });
    let mut window_builder =
        WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title(window_title)
            .inner_size(1280.0, 800.0)
            .min_inner_size(960.0, 600.0)
            .on_navigation(move |target| {
                let origin = navigation_origin
                    .read()
                    .unwrap_or_else(|error| error.into_inner());
                match decide_navigation(origin.as_ref(), target) {
                    NavigationDecision::Allow => {
                        log_app(&format!(
                            "webview_navigation decision=allow {}",
                            safe_target_description(target)
                        ));
                        true
                    }
                    NavigationDecision::OpenExternal => {
                        log_app(&format!(
                            "webview_navigation decision=external_browser {}",
                            safe_target_description(target)
                        ));
                        if let Err(error) = open_external_url(target) {
                            log_error(&format!(
                                "external browser navigation failed: {error}; {}",
                                safe_target_description(target)
                            ));
                        }
                        false
                    }
                    NavigationDecision::Deny => {
                        log_error(&format!(
                            "webview_navigation decision=deny {}",
                            safe_target_description(target)
                        ));
                        false
                    }
                }
            })
            .on_new_window(move |target, _features| {
                let origin = new_window_origin
                    .read()
                    .unwrap_or_else(|error| error.into_inner());
                match decide_new_window(origin.as_ref(), &target) {
                    NavigationDecision::OpenExternal => {
                        log_app(&format!(
                            "webview_new_window decision=external_browser {}",
                            safe_target_description(&target)
                        ));
                        if let Err(error) = open_external_url(&target) {
                            log_error(&format!(
                                "external browser new-window request failed: {error}; {}",
                                safe_target_description(&target)
                            ));
                        }
                        NewWindowResponse::Deny
                    }
                    NavigationDecision::Allow | NavigationDecision::Deny => {
                        log_error(&format!(
                            "webview_new_window decision=deny {}",
                            safe_target_description(&target)
                        ));
                        NewWindowResponse::Deny
                    }
                }
            });
    if let Some(data_directory) = test_webview_data_directory()? {
        window_builder = window_builder.data_directory(data_directory);
    }
    let window = match window_builder.build() {
        Ok(window) => window,
        Err(error) => {
            if let Some(worker) = background_plugin_prepare.take() {
                let _ = worker.join();
            }
            log_error(&format!("main WebView creation failed: {error}"));
            return Err(error.into());
        }
    };
    configure_close_to_tray(&window);
    create_tray(app)?;

    let selection = match selection {
        Ok(selection) => selection,
        Err(message) => {
            fail(&handle, &message);
            return Ok(());
        }
    };
    let mut runtime = selection.primary;
    app.state::<plugin_recovery::PluginRecoveryState>()
        .set_runtime(runtime.clone());
    #[cfg(windows)]
    let directory_picker_owner = window.hwnd().ok().map(|hwnd| hwnd.0 as usize);
    #[cfg(not(windows))]
    let directory_picker_owner = None;
    app.manage(HostSupervisor::with_directory_picker_owner(
        directory_picker_owner,
    ));
    let mut prevalidated_candidate = None;
    if runtime
        .activation
        .as_ref()
        .is_some_and(|activation| activation.candidate)
    {
        match start_and_activate_candidate(&handle, &runtime) {
            Ok(ready) => {
                log_boot_phase(
                    &boot_id,
                    "candidate",
                    "candidate_promoted",
                    runtime_started.elapsed(),
                );
                prevalidated_candidate = Some(ready);
            }
            Err(candidate_error) => {
                log_error(&format!(
                    "runtime candidate failed validation; falling back to active runtime: {candidate_error}"
                ));
                handle.state::<HostSupervisor>().shutdown_for_recovery();
                let Some(fallback) = selection.fallback else {
                    // 首次安装没有 active 可回退时保留 candidate，避免下一次启动误走
                    // payload 安装器不再携带的 legacy 资源目录。
                    fail(
                        &handle,
                        &format!(
                            "the provisioned runtime failed validation and no previous runtime is available: {candidate_error}"
                        ),
                    );
                    return Ok(());
                };
                if let Some(activation) = &runtime.activation {
                    if let Ok(state) = read_runtime_state(&activation.runtime_root) {
                        if state.candidate.as_ref().is_some_and(|candidate| {
                            candidate.payload_digest == activation.payload_digest
                        }) {
                            if let Err(reject_error) = reject_candidate(
                                &activation.runtime_root,
                                &activation.payload_digest,
                            ) {
                                fail(
                                    &handle,
                                    &format!(
                                        "{candidate_error}; candidate rejection failed: {reject_error}"
                                    ),
                                );
                                return Ok(());
                            }
                        }
                    }
                }
                runtime = fallback;
                app.state::<plugin_recovery::PluginRecoveryState>()
                    .set_runtime(runtime.clone());
            }
        }
    }

    let spawn_started = Instant::now();
    let (receiver, initial_ready, plugin_transaction, plugin_degraded_reason) = if let Some((
        receiver,
        lifecycle,
        ready_url,
    )) =
        prevalidated_candidate.take()
    {
        (receiver, Some((lifecycle, ready_url)), None, None)
    } else {
        let mut plugin_degraded_reason = None;
        let plugin_started = Instant::now();
        let (plugin_result, plugin_duration) = match background_plugin_prepare.take() {
            Some(worker) => match worker.join() {
                Ok(result) => result,
                Err(_) => (
                    Err("managed plugin preparation worker panicked".to_owned()),
                    plugin_started.elapsed(),
                ),
            },
            None => {
                let result = PluginManager::new(&runtime).prepare();
                (result, plugin_started.elapsed())
            }
        };
        let mut plugin_transaction = match plugin_result {
            Ok(transaction) => Some(transaction),
            Err(message) => {
                log_error(&format!(
                    "managed plugins were disabled before host startup: {message}"
                ));
                plugin_degraded_reason = Some(message);
                None
            }
        };
        log_boot_phase(&boot_id, "managed", "plugins_prepared", plugin_duration);
        let receiver = match start_host_streams(&handle, &runtime) {
            Ok(receiver) => receiver,
            Err(plugin_error) if plugin_transaction.is_some() => {
                log_error(&format!(
                    "host failed with managed plugins; rolling back before one core retry: {plugin_error}"
                ));
                plugin_degraded_reason = Some(plugin_error.clone());
                if let Some(transaction) = plugin_transaction.take() {
                    if let Err(rollback) = transaction.rollback() {
                        fail(
                            &handle,
                            &format!("{plugin_error}; plugin rollback failed: {rollback}"),
                        );
                        return Ok(());
                    }
                }
                if let Err(repair) = repair_skin_patch_before_core_retry(&runtime) {
                    fail(
                        &handle,
                        &format!("{plugin_error}; skin patch repair failed: {repair}"),
                    );
                    return Ok(());
                }
                match start_host_streams(&handle, &runtime) {
                    Ok(receiver) => receiver,
                    Err(message) => {
                        fail(&handle, &message);
                        return Ok(());
                    }
                }
            }
            Err(message) => {
                fail(&handle, &message);
                return Ok(());
            }
        };
        (receiver, None, plugin_transaction, plugin_degraded_reason)
    };
    log_boot_phase(
        &boot_id,
        if plugin_transaction.is_some() {
            "managed"
        } else {
            "core"
        },
        "host_spawn",
        spawn_started.elapsed(),
    );
    spawn_boot_coordinator(BootCoordinatorInputs {
        handle,
        initial_receiver: receiver,
        initial_ready,
        runtime,
        host_origin,
        plugin_transaction,
        plugin_degraded_reason,
        host_commands,
        boot_id,
        boot_started,
    });
    Ok(())
}

/// 用真实 Host 和插件 readiness 验证 candidate，并原子提交 runtime 与插件状态。
fn start_and_activate_candidate(
    handle: &AppHandle,
    runtime: &RuntimePaths,
) -> Result<(mpsc::Receiver<HostEvent>, LifecycleStateMachine, String), String> {
    let activation = runtime
        .activation
        .as_ref()
        .filter(|activation| activation.candidate)
        .ok_or_else(|| "candidate activation metadata is missing".to_owned())?;
    let mut transaction = PluginManager::new(runtime).prepare()?;
    let receiver = start_host_streams(handle, runtime)?;
    let (mut lifecycle, ready_url) = await_host_ready(
        handle,
        &receiver,
        runtime.core_ready_timeout.max(CANDIDATE_CORE_READY_TIMEOUT),
    )?;
    await_plugins_ready(
        handle,
        &receiver,
        &mut lifecycle,
        runtime
            .plugin_ready_timeout
            .max(CANDIDATE_PLUGIN_READY_TIMEOUT),
    )?;

    if transaction.should_seed_sidebar() {
        let origin = ready_url
            .parse::<url::Url>()
            .map_err(|error| format!("invalid candidate ready URL: {error}"))?;
        sidebar_settings::initialize_sidebar_defaults(&origin)?;
        transaction.mark_sidebar_seeded();
    }

    let previous_state = promote_candidate(&activation.runtime_root, &activation.payload_digest)?;
    if let Err(commit_error) = transaction.commit() {
        return match rollback_candidate_promotion(
            &activation.runtime_root,
            &activation.payload_digest,
            &previous_state,
        ) {
            Ok(()) => Err(format!(
                "candidate plugin transaction commit failed: {commit_error}"
            )),
            Err(rollback_error) => Err(format!(
                "candidate plugin transaction commit failed: {commit_error}; runtime state rollback failed: {rollback_error}"
            )),
        };
    }
    Ok((receiver, lifecycle, ready_url))
}

/// 启动当前 supervisor 中的 Host，并为本次 PID 创建独立事件通道。
fn start_host_streams(
    handle: &AppHandle,
    runtime: &RuntimePaths,
) -> Result<mpsc::Receiver<HostEvent>, String> {
    let supervisor = handle.state::<HostSupervisor>();
    let (stdout, stderr) = supervisor.start(runtime)?;
    let pid = supervisor
        .pid()
        .ok_or_else(|| "host PID is not available after startup".to_owned())?;
    let (sender, receiver) = mpsc::channel::<HostEvent>();
    spawn_stdout_reader(stdout, sender.clone());
    spawn_stderr_reader(stderr);
    spawn_exit_watcher(handle.clone(), sender, pid);
    Ok(receiver)
}

/// core retry 前再次执行已知 Skin patch 迁移，避免回滚或运行期写入留下非法 YAML。
fn repair_skin_patch_before_core_retry(runtime: &RuntimePaths) -> Result<(), String> {
    PluginManager::new(runtime)
        .repair_legacy_skin_patch()
        .map(|_| ())
}

/// 持续排空 Host stdout，并把两级就绪事件发送给启动协调线程。
fn spawn_stdout_reader(stdout: std::process::ChildStdout, sender: mpsc::Sender<HostEvent>) {
    std::thread::spawn(move || {
        let mut parser = ReadinessParser::new();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            log_host(&line);
            match parser.parse_line(&line) {
                Ok(Some(ReadinessSignal::CoreReady(url))) => {
                    let _ = sender.send(HostEvent::CoreReady(url));
                }
                Ok(Some(ReadinessSignal::PluginsReady(url))) => {
                    let _ = sender.send(HostEvent::PluginsReady(url));
                }
                Ok(Some(ReadinessSignal::LegacyReady(url))) => {
                    let _ = sender.send(HostEvent::CoreReady(url.clone()));
                    let _ = sender.send(HostEvent::PluginsReady(url));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = sender.send(HostEvent::ProtocolError(error.to_string()));
                    break;
                }
            }
        }
        if !parser.is_core_ready() {
            let _ = sender.send(HostEvent::Exited(None));
        }
    });
}

/// 持续排空 Host stderr，防止管道塞满后阻塞子进程。
fn spawn_stderr_reader(stderr: std::process::ChildStderr) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            log_host(&line);
        }
    });
}

/// 轮询 Host 退出状态，并在结束时通知启动协调线程。
fn spawn_exit_watcher(handle: AppHandle, sender: mpsc::Sender<HostEvent>, expected_pid: u32) {
    std::thread::spawn(move || loop {
        let supervisor = handle.state::<HostSupervisor>();
        if supervisor.pid() != Some(expected_pid) {
            return;
        }
        if let Some(exit_code) = supervisor.try_exit_code() {
            let _ = sender.send(HostEvent::Exited(exit_code));
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    });
}

/// 启动协调线程所需的完整上下文，避免参数位置错误并保持单一所有权转移。
struct BootCoordinatorInputs {
    handle: AppHandle,
    initial_receiver: mpsc::Receiver<HostEvent>,
    initial_ready: Option<(LifecycleStateMachine, String)>,
    runtime: RuntimePaths,
    host_origin: Arc<RwLock<Option<url::Url>>>,
    plugin_transaction: Option<PluginTransaction>,
    plugin_degraded_reason: Option<String>,
    host_commands: mpsc::Receiver<HostCommand>,
    boot_id: String,
    boot_started: Instant,
}

/// 等待 Host 首次就绪、导航主窗口，并持续处理后续异常退出。
fn spawn_boot_coordinator(inputs: BootCoordinatorInputs) {
    let BootCoordinatorInputs {
        handle,
        initial_receiver,
        initial_ready,
        runtime,
        host_origin,
        mut plugin_transaction,
        mut plugin_degraded_reason,
        host_commands,
        boot_id,
        boot_started,
    } = inputs;
    std::thread::spawn(move || {
        let mut receiver = initial_receiver;
        let candidate_prevalidated = initial_ready.is_some();
        let (mut lifecycle, mut ready_url) = match initial_ready {
            Some(ready) => ready,
            None => match await_host_ready(&handle, &receiver, runtime.core_ready_timeout) {
                Ok(ready) => ready,
                Err(plugin_error) if plugin_transaction.is_some() => {
                    log_error(&format!(
                    "managed plugin startup failed; restoring profile and retrying core once: {plugin_error}"
                ));
                    plugin_degraded_reason = Some(plugin_error.clone());
                    let rollback_started = Instant::now();
                    handle.state::<HostSupervisor>().shutdown_for_recovery();
                    if let Some(transaction) = plugin_transaction.take() {
                        if let Err(rollback) = transaction.rollback() {
                            fail(
                                &handle,
                                &format!("{plugin_error}; plugin rollback failed: {rollback}"),
                            );
                            return;
                        }
                    }
                    log_boot_phase(&boot_id, "managed", "rollback", rollback_started.elapsed());
                    if let Err(repair) = repair_skin_patch_before_core_retry(&runtime) {
                        fail(
                            &handle,
                            &format!("{plugin_error}; skin patch repair failed: {repair}"),
                        );
                        return;
                    }
                    receiver = match start_host_streams(&handle, &runtime) {
                        Ok(receiver) => receiver,
                        Err(core_error) => {
                            fail(&handle, &core_error);
                            return;
                        }
                    };
                    match await_host_ready(&handle, &receiver, runtime.core_ready_timeout) {
                        Ok(ready) => ready,
                        Err(core_error) => {
                            fail(&handle, &core_error);
                            return;
                        }
                    }
                }
                Err(message) => {
                    fail(&handle, &message);
                    return;
                }
            },
        };

        log_boot_phase(&boot_id, "managed", "core_ready", boot_started.elapsed());
        if let Err(message) = navigate_to_host(&handle, &host_origin, &ready_url) {
            fail(&handle, &message);
            return;
        }
        handle.state::<HostController>().mark_ready();

        let mut plugins_ready = if candidate_prevalidated {
            true
        } else {
            match await_plugins_ready(
                &handle,
                &receiver,
                &mut lifecycle,
                runtime.plugin_ready_timeout,
            ) {
                Ok(()) => true,
                Err(plugin_error) if plugin_transaction.is_some() => {
                    log_error(&format!(
                    "managed plugins failed after core readiness; restoring profile and retrying core once: {plugin_error}"
                ));
                    plugin_degraded_reason = Some(plugin_error.clone());
                    let _ = navigate_to_recovery(&handle, &host_origin);
                    let rollback_started = Instant::now();
                    handle.state::<HostSupervisor>().shutdown_for_recovery();
                    if let Some(transaction) = plugin_transaction.take() {
                        if let Err(rollback) = transaction.rollback() {
                            fail(
                                &handle,
                                &format!("{plugin_error}; plugin rollback failed: {rollback}"),
                            );
                            return;
                        }
                    }
                    log_boot_phase(&boot_id, "managed", "rollback", rollback_started.elapsed());
                    if let Err(repair) = repair_skin_patch_before_core_retry(&runtime) {
                        fail(
                            &handle,
                            &format!("{plugin_error}; skin patch repair failed: {repair}"),
                        );
                        return;
                    }
                    receiver = match start_host_streams(&handle, &runtime) {
                        Ok(receiver) => receiver,
                        Err(core_error) => {
                            fail(&handle, &core_error);
                            return;
                        }
                    };
                    (lifecycle, ready_url) =
                        match await_host_ready(&handle, &receiver, runtime.core_ready_timeout) {
                            Ok(ready) => ready,
                            Err(core_error) => {
                                fail(&handle, &core_error);
                                return;
                            }
                        };
                    log_boot_phase(&boot_id, "core", "core_ready", boot_started.elapsed());
                    if let Err(message) = navigate_to_host(&handle, &host_origin, &ready_url) {
                        fail(&handle, &message);
                        return;
                    }
                    handle.state::<HostController>().mark_ready();
                    match await_plugins_ready(
                        &handle,
                        &receiver,
                        &mut lifecycle,
                        runtime.plugin_ready_timeout,
                    ) {
                        Ok(()) => true,
                        Err(degraded) => {
                            plugin_degraded_reason = Some(degraded);
                            false
                        }
                    }
                }
                Err(plugin_error) => {
                    plugin_degraded_reason = Some(plugin_error);
                    false
                }
            }
        };

        if plugin_transaction
            .as_ref()
            .is_some_and(PluginTransaction::should_seed_sidebar)
        {
            let seed_result = ready_url
                .parse::<url::Url>()
                .map_err(|error| format!("invalid ready URL for sidebar settings: {error}"))
                .and_then(|origin| sidebar_settings::initialize_sidebar_defaults(&origin));
            match seed_result {
                Ok(()) => {
                    if let Some(transaction) = plugin_transaction.as_mut() {
                        transaction.mark_sidebar_seeded();
                    }
                }
                Err(plugin_error) => {
                    log_error(&format!(
                        "Better Sidebar security initialization failed; retrying core without managed plugins: {plugin_error}"
                    ));
                    plugin_degraded_reason = Some(plugin_error.clone());
                    let _ = navigate_to_recovery(&handle, &host_origin);
                    handle.state::<HostSupervisor>().shutdown_for_recovery();
                    if let Some(transaction) = plugin_transaction.take() {
                        if let Err(rollback) = transaction.rollback() {
                            fail(
                                &handle,
                                &format!("{plugin_error}; plugin rollback failed: {rollback}"),
                            );
                            return;
                        }
                    }
                    if let Err(repair) = repair_skin_patch_before_core_retry(&runtime) {
                        fail(
                            &handle,
                            &format!("{plugin_error}; skin patch repair failed: {repair}"),
                        );
                        return;
                    }
                    receiver = match start_host_streams(&handle, &runtime) {
                        Ok(receiver) => receiver,
                        Err(core_error) => {
                            fail(&handle, &core_error);
                            return;
                        }
                    };
                    (lifecycle, ready_url) =
                        match await_host_ready(&handle, &receiver, runtime.core_ready_timeout) {
                            Ok(ready) => ready,
                            Err(core_error) => {
                                fail(&handle, &core_error);
                                return;
                            }
                        };
                    if let Err(message) = navigate_to_host(&handle, &host_origin, &ready_url) {
                        fail(&handle, &message);
                        return;
                    }
                    handle.state::<HostController>().mark_ready();
                    plugins_ready = await_plugins_ready(
                        &handle,
                        &receiver,
                        &mut lifecycle,
                        runtime.plugin_ready_timeout,
                    )
                    .is_ok();
                }
            }
        }

        if plugins_ready {
            log_boot_phase(&boot_id, "managed", "plugins_ready", boot_started.elapsed());
        } else {
            log_boot_phase(&boot_id, "core", "plugins_degraded", boot_started.elapsed());
        }
        if plugins_ready {
            if let Some(transaction) = plugin_transaction.take() {
                if let Err(message) = transaction.commit() {
                    handle.state::<HostSupervisor>().shutdown_for_recovery();
                    fail(
                        &handle,
                        &format!("failed to commit managed plugins: {message}"),
                    );
                    return;
                }
            }
        }

        log_app(&format!(
            "host ready: {ready_url} (started in {} ms)",
            boot_started.elapsed().as_millis()
        ));
        if let Some(reason) = plugin_degraded_reason {
            let _ = handle
                .dialog()
                .message(format!(
                    "内置插件本次未启用，DSH 核心已降级启动。\n\n{reason}\n\n日志：{}",
                    log_file_path().display()
                ))
                .title("DeepSeek Harness Desktop 插件降级")
                .blocking_show();
        }

        let mut active_receiver = Some(receiver);
        loop {
            let shutting_down = handle.state::<desktop::DesktopLifecycle>().is_quitting();
            if shutting_down {
                handle.state::<HostController>().mark_stopping();
                return;
            }

            match host_commands.try_recv() {
                Ok(HostCommand::Restart) => {
                    match restart_host(&handle, &runtime, &host_origin) {
                        Ok((next_receiver, next_lifecycle)) => {
                            active_receiver = Some(next_receiver);
                            lifecycle = next_lifecycle;
                            handle.state::<HostController>().mark_ready();
                        }
                        Err(message) => {
                            active_receiver = None;
                            handle.state::<HostController>().mark_failed();
                            report_restart_failure(&handle, &message);
                        }
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {}
            }

            let Some(current_receiver) = active_receiver.as_ref() else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };
            let event = match current_receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            match lifecycle.on_event(event, false) {
                LifecycleAction::Fail { message, .. } => {
                    handle.state::<HostController>().mark_failed();
                    fail(&handle, &message);
                    return;
                }
                LifecycleAction::Ignore
                | LifecycleAction::Navigate(_)
                | LifecycleAction::PluginsReady => {}
                LifecycleAction::PluginDegraded { message } => {
                    log_error(&format!("runtime plugin degradation: {message}"));
                }
            }
        }
    });
}

/// 串行停止当前 Host、启动新实例、等待就绪并把 WebView 导航到新端口。
fn restart_host(
    handle: &AppHandle,
    runtime: &RuntimePaths,
    host_origin: &Arc<RwLock<Option<url::Url>>>,
) -> Result<(mpsc::Receiver<HostEvent>, LifecycleStateMachine), String> {
    let previous_pid = handle.state::<HostSupervisor>().pid();
    log_app(&format!("restarting host: previous_pid={previous_pid:?}"));
    handle.state::<HostSupervisor>().shutdown();
    repair_skin_patch_before_core_retry(runtime)?;

    let transaction = PluginManager::new(runtime).prepare()?;
    let (receiver, lifecycle, ready_url) = coordinate_managed_restart(
        transaction,
        || {
            let receiver = start_host_streams(handle, runtime)?;
            let (mut lifecycle, ready_url) =
                await_host_ready(handle, &receiver, runtime.core_ready_timeout)?;
            navigate_to_host(handle, host_origin, &ready_url)?;
            await_plugins_ready(
                handle,
                &receiver,
                &mut lifecycle,
                runtime.plugin_ready_timeout,
            )?;
            Ok((receiver, lifecycle, ready_url))
        },
        |plugin_error| {
            log_error(&format!(
                "managed plugin restart failed; restoring profile and retrying once: {plugin_error}"
            ));
            let _ = navigate_to_recovery(handle, host_origin);
            handle.state::<HostSupervisor>().shutdown_for_recovery();
            repair_skin_patch_before_core_retry(runtime)?;

            let receiver = start_host_streams(handle, runtime)?;
            let (mut lifecycle, ready_url) =
                await_host_ready(handle, &receiver, runtime.core_ready_timeout)?;
            navigate_to_host(handle, host_origin, &ready_url)?;
            await_plugins_ready(
                handle,
                &receiver,
                &mut lifecycle,
                runtime.plugin_ready_timeout,
            )?;
            Ok((receiver, lifecycle, ready_url))
        },
    )?;
    log_app(&format!(
        "host restart ready: pid={:?}, url={ready_url}",
        handle.state::<HostSupervisor>().pid()
    ));
    Ok((receiver, lifecycle))
}

/// 抽象重启使用的插件事务，使托盘重启顺序可以脱离 Tauri 窗口进行单元测试。
trait RestartPluginTransaction {
    /// Host 和插件全部就绪后提交本轮 profile 与链接修改。
    fn commit(self) -> Result<(), String>;
    /// Host 启动失败时恢复本轮 profile 与链接修改。
    fn rollback(self) -> Result<(), String>;
}

impl RestartPluginTransaction for PluginTransaction {
    fn commit(self) -> Result<(), String> {
        PluginTransaction::commit(self)
    }

    fn rollback(self) -> Result<(), String> {
        PluginTransaction::rollback(self)
    }
}

/// 执行一次受管重启；失败时必须先完成插件回滚，再调用唯一一次恢复启动。
fn coordinate_managed_restart<T, R, Start, Recover>(
    transaction: T,
    start: Start,
    recover: Recover,
) -> Result<R, String>
where
    T: RestartPluginTransaction,
    Start: FnOnce() -> Result<R, String>,
    Recover: FnOnce(&str) -> Result<R, String>,
{
    match start() {
        Ok(ready) => {
            transaction.commit()?;
            Ok(ready)
        }
        Err(plugin_error) => {
            transaction.rollback().map_err(|rollback| {
                format!("{plugin_error}; plugin rollback failed: {rollback}")
            })?;
            recover(&plugin_error)
        }
    }
}

/// 将 WebView 切回内置恢复页，避免回滚时继续停留在即将失效的 Host 端口。
fn navigate_to_recovery(
    handle: &AppHandle,
    host_origin: &Arc<RwLock<Option<url::Url>>>,
) -> Result<(), String> {
    *host_origin
        .write()
        .unwrap_or_else(|error| error.into_inner()) = None;
    let recovery = "http://tauri.localhost/index.html"
        .parse::<url::Url>()
        .map_err(|error| format!("invalid recovery URL: {error}"))?;
    if let Some(window) = handle.get_webview_window("main") {
        window
            .navigate(recovery)
            .map_err(|error| format!("failed to navigate WebView to recovery page: {error}"))?;
    }
    Ok(())
}

/// 更新允许导航的 Host 原点，并将主窗口切换到新实例的实际地址。
fn navigate_to_host(
    handle: &AppHandle,
    host_origin: &Arc<RwLock<Option<url::Url>>>,
    ready_url: &str,
) -> Result<(), String> {
    let parsed = ready_url
        .parse::<url::Url>()
        .map_err(|error| format!("invalid Host ready URL: {error}"))?;
    *host_origin
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(parsed.clone());
    if let Some(window) = handle.get_webview_window("main") {
        window
            .navigate(parsed)
            .map_err(|error| format!("failed to navigate WebView to restarted Host: {error}"))?;
    }
    Ok(())
}

/// 记录重启失败并保留桌面托盘，使用户可以修复环境后再次尝试。
fn report_restart_failure(handle: &AppHandle, message: &str) {
    log_error(&format!("host restart failed: {message}"));
    let _ = handle
        .dialog()
        .message(format!(
            "DSH 服务重启失败。可以从托盘再次重试。\n\n{message}\n\n日志：{}",
            log_file_path().display()
        ))
        .title("DSH 服务重启失败")
        .blocking_show();
}

/// 等待一个 Host 实例首次就绪，并返回后续事件需要复用的状态机。
fn await_host_ready(
    handle: &AppHandle,
    receiver: &mpsc::Receiver<HostEvent>,
    readiness_timeout: Duration,
) -> Result<(LifecycleStateMachine, String), String> {
    let mut lifecycle = LifecycleStateMachine::new();
    let shutting_down = handle.state::<desktop::DesktopLifecycle>().is_quitting();
    let action = match receiver.recv_timeout(readiness_timeout) {
        Ok(event) => lifecycle.on_event(event, shutting_down),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            lifecycle.on_timeout(shutting_down, readiness_timeout.as_secs())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err("host event channel disconnected before readiness".to_owned())
        }
    };
    match action {
        LifecycleAction::Navigate(url) => Ok((lifecycle, url)),
        LifecycleAction::Fail { message, .. } => Err(message),
        LifecycleAction::Ignore => Err("host startup was cancelled".to_owned()),
        LifecycleAction::PluginsReady => {
            Err("host reported plugins before core readiness".to_owned())
        }
        LifecycleAction::PluginDegraded { message } => Err(message),
    }
}

/// 在核心页面已可用后等待全部 Loader 插件完成，超时仅返回可降级错误。
fn await_plugins_ready(
    handle: &AppHandle,
    receiver: &mpsc::Receiver<HostEvent>,
    lifecycle: &mut LifecycleStateMachine,
    plugin_timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + plugin_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return match lifecycle.on_plugins_timeout(
                handle.state::<desktop::DesktopLifecycle>().is_quitting(),
                plugin_timeout.as_secs(),
            ) {
                LifecycleAction::PluginDegraded { message } => Err(message),
                _ => Err("host plugin readiness wait was cancelled".to_owned()),
            };
        }
        let event = match receiver.recv_timeout(remaining) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("host event channel disconnected before plugins were ready".to_owned())
            }
        };
        match lifecycle.on_event(
            event,
            handle.state::<desktop::DesktopLifecycle>().is_quitting(),
        ) {
            LifecycleAction::PluginsReady => return Ok(()),
            LifecycleAction::Fail { message, .. } | LifecycleAction::PluginDegraded { message } => {
                return Err(message)
            }
            LifecycleAction::Ignore | LifecycleAction::Navigate(_) => {}
        }
    }
}

/// 记录稳定 key-value 启动阶段，供单次启动追踪和 P95 基准脚本聚合。
fn log_boot_phase(boot_id: &str, attempt: &str, phase: &str, duration: Duration) {
    log_app(&format!(
        "boot_id={boot_id} phase={phase} duration_ms={} attempt={attempt}",
        duration.as_millis()
    ));
}

/// 记录启动错误并打开独立恢复页；桌面进程保持存活等待用户处理。
fn fail(handle: &AppHandle, message: &str) {
    if let Some(controller) = handle.try_state::<HostController>() {
        controller.mark_failed();
    }
    log_error(message);
    if let Some(supervisor) = handle.try_state::<HostSupervisor>() {
        supervisor.shutdown_for_recovery();
    }
    plugin_recovery::show_failure_recovery(handle, message);
}

#[cfg(test)]
mod restart_tests {
    use std::sync::{Arc, Mutex};

    use super::{coordinate_managed_restart, RestartPluginTransaction};

    struct RecordingTransaction {
        events: Arc<Mutex<Vec<&'static str>>>,
        rollback_error: Option<String>,
    }

    impl RestartPluginTransaction for RecordingTransaction {
        fn commit(self) -> Result<(), String> {
            self.events.lock().unwrap().push("commit");
            Ok(())
        }

        fn rollback(self) -> Result<(), String> {
            self.events.lock().unwrap().push("rollback");
            self.rollback_error.map_or(Ok(()), Err)
        }
    }

    #[test]
    fn managed_restart_commits_only_after_host_is_ready() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let transaction = RecordingTransaction {
            events: events.clone(),
            rollback_error: None,
        };

        let result = coordinate_managed_restart(
            transaction,
            || {
                events.lock().unwrap().push("start");
                Ok("ready")
            },
            |_| {
                events.lock().unwrap().push("retry");
                Ok("recovered")
            },
        )
        .unwrap();

        assert_eq!(result, "ready");
        assert_eq!(*events.lock().unwrap(), vec!["start", "commit"]);
    }

    #[test]
    fn managed_restart_rolls_back_before_single_recovery_retry() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let transaction = RecordingTransaction {
            events: events.clone(),
            rollback_error: None,
        };

        let result = coordinate_managed_restart(
            transaction,
            || {
                events.lock().unwrap().push("start");
                Err("plugins failed".to_owned())
            },
            |error| {
                assert_eq!(error, "plugins failed");
                events.lock().unwrap().push("retry");
                Ok("recovered")
            },
        )
        .unwrap();

        assert_eq!(result, "recovered");
        assert_eq!(*events.lock().unwrap(), vec!["start", "rollback", "retry"]);
    }

    #[test]
    fn managed_restart_does_not_retry_when_rollback_fails() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let transaction = RecordingTransaction {
            events: events.clone(),
            rollback_error: Some("restore failed".to_owned()),
        };

        let error = coordinate_managed_restart(
            transaction,
            || Err::<(), _>("plugins failed".to_owned()),
            |_| {
                events.lock().unwrap().push("retry");
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(
            error,
            "plugins failed; plugin rollback failed: restore failed"
        );
        assert_eq!(*events.lock().unwrap(), vec!["rollback"]);
    }
}
