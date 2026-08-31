//! 主窗口、系统托盘和显式退出行为。

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::DialogExt;

use crate::lifecycle::HostController;
use crate::logger::{log_app, log_error, log_file_path};
use crate::navigation::is_external_browser_url;

const MENU_OPEN: &str = "open-main";
const MENU_RESTART: &str = "restart-host";
const MENU_LOG: &str = "open-log";
const MENU_WEBSITE: &str = "open-project-website";
const MENU_FEEDBACK: &str = "open-feedback";
const MENU_CHECK_UPDATE: &str = "check-update";
const MENU_ABOUT: &str = "about";
const MENU_QUIT: &str = "quit";

const PROJECT_WEBSITE_URL: &str = "https://dsh.cubee.chat/";
const FEEDBACK_URL: &str =
    "https://github.com/licn9901-arch/deepseek-harness-desktop/issues/new/choose";
const UPDATE_CHECK_URL: &str = "https://dsh.cubee.chat/download/windows/";

/// 保存用户是否已经选择显式退出，区分隐藏窗口与结束应用。
#[derive(Default)]
pub struct DesktopLifecycle {
    quitting: AtomicBool,
}

impl DesktopLifecycle {
    /// 标记显式退出；重复调用保持幂等。
    pub fn request_quit(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    /// 返回当前是否正在显式退出。
    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}

/// 为主窗口注册关闭到托盘行为。
pub fn configure_close_to_tray(window: &WebviewWindow) {
    let window_for_event = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            let lifecycle = window_for_event.state::<DesktopLifecycle>();
            if !lifecycle.is_quitting() {
                api.prevent_close();
                let _ = window_for_event.hide();
                log_app("main window hidden to tray");
            }
        }
    });
}

/// 创建带主窗口、Host、项目入口、日志、关于和退出命令的原生托盘。
pub fn create_tray(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "打开主窗口", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, MENU_RESTART, "重启 DSH 服务", true, None::<&str>)?;
    let log = MenuItem::with_id(app, MENU_LOG, "打开日志", true, None::<&str>)?;
    let website = MenuItem::with_id(app, MENU_WEBSITE, "项目官网", true, None::<&str>)?;
    let feedback = MenuItem::with_id(app, MENU_FEEDBACK, "反馈问题", true, None::<&str>)?;
    let check_update = MenuItem::with_id(app, MENU_CHECK_UPDATE, "检查更新", true, None::<&str>)?;
    let about = MenuItem::with_id(app, MENU_ABOUT, "关于", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &restart,
            &log,
            &website,
            &feedback,
            &check_update,
            &about,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("DeepSeek Harness Desktop")
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_RESTART => request_host_restart(app),
            MENU_LOG => open_log_file(),
            MENU_WEBSITE | MENU_FEEDBACK | MENU_CHECK_UPDATE => {
                open_tray_external_target(event.id().as_ref())
            }
            MENU_ABOUT => show_about(app),
            MENU_QUIT => quit_application(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 将托盘重启请求交给桌面 Host 控制器，状态冲突时向用户给出明确提示。
fn request_host_restart(app: &AppHandle) {
    let Some(controller) = app.try_state::<HostController>() else {
        log_error("host controller is unavailable");
        return;
    };
    match controller.restart() {
        Ok(()) => log_app("host restart requested from tray"),
        Err(message) => {
            log_error(&format!("host restart request rejected: {message}"));
            let _ = app
                .dialog()
                .message("DSH 服务当前正在启动、重启或退出，请稍后再试。")
                .title("无法重启 DSH 服务")
                .blocking_show();
        }
    }
}

/// 恢复主窗口并将输入焦点交给它，供托盘和二次启动复用。
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 标记显式退出并触发 Tauri 退出流程，Host 由统一退出回调清理。
pub fn quit_application(app: &AppHandle) {
    app.state::<DesktopLifecycle>().request_quit();
    log_app("explicit application exit requested");
    app.exit(0);
}

/// 把通过二次白名单校验的外部 HTTP/HTTPS 地址交给 Windows 默认浏览器。
pub fn open_external_url(url: &url::Url) -> Result<(), String> {
    if !is_external_browser_url(url) {
        return Err("refusing to open an internal or non-HTTP URL externally".to_owned());
    }
    Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url.as_str()])
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open the system browser: {error}"))
}

/// 返回已登记托盘入口对应的编译期 URL，未知菜单项不会产生外部导航。
fn tray_external_target(menu_id: &str) -> Option<&'static str> {
    match menu_id {
        MENU_WEBSITE => Some(PROJECT_WEBSITE_URL),
        MENU_FEEDBACK => Some(FEEDBACK_URL),
        MENU_CHECK_UPDATE => Some(UPDATE_CHECK_URL),
        _ => None,
    }
}

/// 校验并打开托盘登记的外部地址；解析或浏览器启动失败只写入脱敏日志。
fn open_tray_external_target(menu_id: &str) {
    let Some(target) = tray_external_target(menu_id) else {
        return;
    };
    match url::Url::parse(target).map_err(|error| error.to_string()) {
        Ok(url) => {
            if let Err(error) = open_external_url(&url) {
                log_error(&format!("tray external navigation failed: {error}"));
            }
        }
        Err(error) => log_error(&format!("invalid compiled tray URL: {error}")),
    }
}

/// 使用资源管理器定位日志文件，避免把日志内容暴露给 WebView。
fn open_log_file() {
    let path = log_file_path();
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    let argument = format!("/select,{}", path.display());
    let _ = Command::new("explorer.exe").arg(argument).spawn();
}

/// 显示版本与非官方声明，不向远程页面暴露任何命令。
fn show_about(app: &AppHandle) {
    let _ = app
        .dialog()
        .message(format!(
            "DeepSeek Harness Desktop {}\n\n社区项目，非 DeepSeek 官方产品。\n\n内置 DSH 0.1.2-alpha.2、DSH Market 1.38.1、pnpm 10.34.5\n插件：GenUI 0.9.6、Better Sidebar 0.17.1、Skin Center 0.3.10、Hindsight 0.4.3、Skills/MCP 0.2.4\n\nDSH 仅在选用支持图片的原生多模态模型时提供视觉能力；纯文本模型没有桌面视觉回退。第三方插件与桌面应用拥有相同主机权限，目前没有签名验证、权限清单或进程级沙箱。",
            app.package_info().version
        ))
        .title("关于 DeepSeek Harness Desktop")
        .blocking_show();
}

#[cfg(test)]
mod tests {
    use super::{tray_external_target, MENU_CHECK_UPDATE, MENU_FEEDBACK, MENU_WEBSITE};

    /// 验证托盘外部入口只返回编译期登记的 HTTPS 地址，避免菜单事件接收任意 URL。
    #[test]
    fn tray_external_targets_are_fixed_https_urls() {
        for menu_id in [MENU_WEBSITE, MENU_FEEDBACK, MENU_CHECK_UPDATE] {
            let target = tray_external_target(menu_id).expect("已登记的托盘入口必须存在");
            let parsed = url::Url::parse(target).expect("编译期 URL 必须合法");
            assert_eq!(parsed.scheme(), "https");
        }
    }

    /// 验证未知菜单项不会被转化为外部浏览器导航。
    #[test]
    fn unknown_tray_item_has_no_external_target() {
        assert_eq!(tray_external_target("unregistered-item"), None);
    }
}
