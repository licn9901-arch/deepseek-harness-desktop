import { invoke } from "@tauri-apps/api/core";

const elements = {
  failure: document.querySelector("#failure"),
  failureMessage: document.querySelector("#failure-message"),
  logPath: document.querySelector("#log-path"),
  plugins: document.querySelector("#plugins"),
  relaunch: document.querySelector("#relaunch"),
  status: document.querySelector("#status"),
};

let busy = false;
let snapshot;

function sourceLabel(source) {
  return { system: "系统", builtin: "内置", user: "Market / 用户" }[source] ?? source;
}

function node(tag, className, text) {
  const value = document.createElement(tag);
  if (className) value.className = className;
  if (text !== undefined) value.textContent = text;
  return value;
}

function actionButton(label, className, action, disabled = false) {
  const button = node("button", className, label);
  button.type = "button";
  button.disabled = disabled || busy;
  button.addEventListener("click", action);
  return button;
}

async function mutate(command, args, pendingMessage) {
  if (busy) return;
  busy = true;
  elements.status.textContent = pendingMessage;
  renderPlugins();
  try {
    await invoke(command, args);
    await load();
  } catch (error) {
    elements.status.textContent = String(error);
  } finally {
    busy = false;
    renderPlugins();
  }
}

function renderPlugins() {
  elements.plugins.replaceChildren();
  for (const plugin of snapshot?.plugins ?? []) {
    const card = node("article", "plugin");
    const info = node("div");
    const title = node("div", "plugin-title");
    title.append(node("h3", "", plugin.label));
    const badge = node(
      "span",
      `badge ${plugin.protected ? "system" : plugin.enabled ? "" : "off"}`,
      plugin.protected ? "受保护" : plugin.enabled ? "已启用" : "已禁用",
    );
    title.append(badge);
    info.append(title, node("div", "package", plugin.package));
    info.append(node("p", "meta", `${sourceLabel(plugin.source)} · ${plugin.version}${plugin.installed ? "" : " · 文件缺失"}`));
    if (plugin.issue) info.append(node("p", "issue", plugin.issue));

    const actions = node("div", "actions");
    if (!plugin.protected) {
      actions.append(actionButton(
        plugin.enabled ? "禁用" : "启用",
        "",
        () => mutate(
          "recovery_plugin_set_enabled",
          { package: plugin.package, enabled: !plugin.enabled },
          `${plugin.enabled ? "正在禁用" : "正在启用"} ${plugin.label}…`,
        ),
        !plugin.enabled && !plugin.installed,
      ));
    }
    if (plugin.canUninstall) {
      actions.append(actionButton("卸载", "danger", () => {
        if (window.confirm(`确定卸载 ${plugin.label}（${plugin.package}）吗？\n\n插件会先被禁用；卸载失败时仍保持禁用。`)) {
          void mutate("recovery_plugin_uninstall", { package: plugin.package }, `正在卸载 ${plugin.label}…`);
        }
      }));
    }
    card.append(info, actions);
    elements.plugins.append(card);
  }
  elements.plugins.setAttribute("aria-busy", String(busy));
}

async function load() {
  snapshot = await invoke("recovery_plugin_list");
  elements.failure.hidden = !snapshot.failure;
  elements.failureMessage.textContent = snapshot.failure ?? "";
  elements.logPath.textContent = `日志：${snapshot.logPath}`;
  elements.relaunch.disabled = busy;
  elements.status.textContent = snapshot.restartRequired
    ? "插件状态已变更，需要重启整个桌面应用。"
    : `已读取 ${snapshot.plugins.length} 个组件。`;
  renderPlugins();
}

elements.relaunch.addEventListener("click", async () => {
  if (busy) return;
  busy = true;
  elements.relaunch.disabled = true;
  elements.status.textContent = "正在退出并重新启动桌面应用…";
  try {
    await invoke("recovery_relaunch");
  } catch (error) {
    busy = false;
    elements.relaunch.disabled = false;
    elements.status.textContent = String(error);
  }
});

load().catch((error) => {
  elements.status.textContent = `无法读取插件：${String(error)}`;
});
