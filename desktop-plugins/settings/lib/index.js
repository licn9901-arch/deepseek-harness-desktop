import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, isAbsolute, join } from "node:path";

const API_PREFIX = "/api/desktop-managed-plugins";
const HINDSIGHT_API_PREFIX = "/api/desktop-hindsight";
const HINDSIGHT_CREDENTIAL_REF = "DSH_DESKTOP_HINDSIGHT_API_TOKEN";
const HINDSIGHT_CLOUD_URL = "https://api.hindsight.vectorize.io";
const CONTROL_BUNDLE = "@dsh-desktop/settings";
const PROTECTED_BUNDLES = new Set([
  "@deepseek-ai/dsh-base",
  "@deepseek-ai/dsh-web-app",
  "dshmarket",
  "@dsh-desktop/runtime-services",
]);
const MANAGED_PLUGINS = Object.freeze([
  { package: "@changfenhuang/dsh-genui", label: "GenUI" },
  { package: "dsh-better-sidebar", label: "Better Sidebar" },
  { package: "@linxin666/dsh-client-ui-skin-center", label: "主题皮肤" },
  { package: "@vectorize-io/hindsight-coding-agents", label: "Hindsight 记忆" },
  { package: "@cubee-slide/skills-mcp-manager", label: "Skills / MCP Manager" },
]);
const TOGGLEABLE = new Set(MANAGED_PLUGINS.map((item) => item.package));
const RETIRED_THEME_BUNDLES = new Set([
  "@linxin666/dsh-skins",
]);
let writeTail = Promise.resolve();
let temporarySequence = 0;

function json(res, status, body) {
  res.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(body));
}

function requireMethod(req, res, method) {
  if (req.method === method) return true;
  json(res, 405, { ok: false, error: "method-not-allowed" });
  return false;
}

/** 拒绝浏览器跨站请求，避免任意网页借 localhost 修改用户 profile。 */
function isSameOriginRequest(req) {
  const host = req.headers.host;
  if (typeof host !== "string" || host === "") return false;
  let hostname;
  try {
    hostname = new URL(`http://${host}`).hostname;
  } catch {
    return false;
  }
  if (!["127.0.0.1", "localhost", "[::1]"].includes(hostname)) return false;
  if (req.headers["sec-fetch-site"] === "cross-site") return false;
  const origin = req.headers.origin;
  if (typeof origin !== "string" || origin === "" || origin === "null") return true;
  try {
    return new URL(origin).host === host;
  } catch {
    return false;
  }
}

function requireSameOrigin(req, res) {
  if (isSameOriginRequest(req)) return true;
  json(res, 403, { ok: false, error: "cross-site-request-rejected" });
  return false;
}

/** 有界读取 JSON，避免本地 API 被超大请求拖垮。 */
function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > 64 * 1024) {
        reject(new Error("body-too-large"));
        queueMicrotask(() => req.destroy());
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      try {
        resolve(chunks.length === 0 ? {} : JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch {
        reject(new Error("invalid-json"));
      }
    });
    req.on("error", reject);
  });
}

function profilePath() {
  return join(process.env.DSH_HOME || join(homedir(), ".dsh"), "profiles", "web", "package.json");
}

function userHome() {
  return process.env.DSH_DESKTOP_USER_HOME || homedir();
}

function hindsightConfigPath() {
  return join(userHome(), ".hindsight", "coding-agent.json");
}

async function readHindsightConfig(path = hindsightConfigPath()) {
  try {
    const value = JSON.parse(await readFile(path, "utf8"));
    if (value === null || Array.isArray(value) || typeof value !== "object") {
      throw new Error("invalid-hindsight-config");
    }
    return value;
  } catch (error) {
    if (error?.code === "ENOENT") return {};
    if (error instanceof SyntaxError) throw new Error("invalid-hindsight-config");
    throw error;
  }
}

function normalizeHindsightConfig(config) {
  const serverMode = config.serverMode === "self-hosted" ? "self-hosted" : "cloud";
  const apiUrl = serverMode === "cloud" ? HINDSIGHT_CLOUD_URL : String(config.apiUrl || "");
  const dsh = config.harnesses?.dsh;
  const optInPaths = Array.isArray(dsh?.optInPaths)
    ? [...new Set(dsh.optInPaths.filter((path) => typeof path === "string" && isAbsolute(path) && !path.includes("\0")))]
    : [];
  return { serverMode, apiUrl, optInPaths };
}

function validateHindsightUrl(serverMode, rawUrl) {
  if (!["cloud", "self-hosted"].includes(serverMode)) throw new Error("invalid-server-mode");
  const value = serverMode === "cloud" ? HINDSIGHT_CLOUD_URL : rawUrl;
  if (typeof value !== "string" || value.length > 2048) throw new Error("invalid-api-url");
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error("invalid-api-url");
  }
  if (!["http:", "https:"].includes(parsed.protocol) || parsed.username !== "" || parsed.password !== "") {
    throw new Error("invalid-api-url");
  }
  return parsed.toString().replace(/\/$/, "");
}

function assertHindsightConfigBody(body) {
  if (body === null || Array.isArray(body) || typeof body !== "object") throw new Error("invalid-request");
  const keys = Object.keys(body).sort().join(",");
  if (keys !== "apiUrl,optInPaths,serverMode" || !Array.isArray(body.optInPaths)) {
    throw new Error("invalid-request");
  }
  const apiUrl = validateHindsightUrl(body.serverMode, body.apiUrl);
  const optInPaths = [...new Set(body.optInPaths.map((path) => {
    if (typeof path !== "string" || !isAbsolute(path) || path.includes("\0")) {
      throw new Error("invalid-opt-in-path");
    }
    return path;
  }))];
  return { serverMode: body.serverMode, apiUrl, optInPaths };
}

function assertCredentialBody(body) {
  if (body === null || Array.isArray(body) || typeof body !== "object") throw new Error("invalid-request");
  const keys = Object.keys(body).sort().join(",");
  if (keys === "clear" && body.clear === true) return { clear: true };
  if (keys !== "token" || typeof body.token !== "string" || body.token.trim() === "" || body.token.length > 16 * 1024) {
    throw new Error("invalid-credential");
  }
  return { clear: false, token: body.token };
}

async function atomicWriteJson(path, value) {
  temporarySequence += 1;
  await mkdir(dirname(path), { recursive: true });
  const temporary = join(dirname(path), `.${path.split(/[\\/]/).at(-1)}.${process.pid}.${temporarySequence}.tmp`);
  try {
    await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, "utf8");
    await rename(temporary, path);
  } catch (error) {
    await unlink(temporary).catch(() => {});
    throw error;
  }
}

async function readHindsightState(credentials, path = hindsightConfigPath()) {
  const config = await readHindsightConfig(path);
  const credential = await credentials.describe(HINDSIGHT_CREDENTIAL_REF);
  return {
    ok: true,
    config: normalizeHindsightConfig(config),
    credential: {
      configured: Boolean(credential.configured || (typeof config.apiToken === "string" && config.apiToken !== "")),
      source: typeof config.apiToken === "string" && config.apiToken !== "" ? "legacy-file" : credential.source,
      writable: Boolean(credential.writable),
    },
  };
}

async function saveHindsightConfig(body, path = hindsightConfigPath()) {
  const next = assertHindsightConfigBody(body);
  return serializeWrite(async () => {
    const config = await readHindsightConfig(path);
    config.serverMode = next.serverMode;
    config.apiUrl = next.apiUrl;
    const harnesses = config.harnesses && !Array.isArray(config.harnesses) && typeof config.harnesses === "object"
      ? config.harnesses
      : {};
    const existingDsh = harnesses.dsh && !Array.isArray(harnesses.dsh) && typeof harnesses.dsh === "object"
      ? harnesses.dsh
      : {};
    config.harnesses = harnesses;
    harnesses.dsh = { ...existingDsh, optInOnly: true, optInPaths: next.optInPaths };
    await atomicWriteJson(path, config);
    return { ok: true, config: next, restartRequired: true };
  });
}

async function updateHindsightCredential(credentials, body, path = hindsightConfigPath()) {
  const input = assertCredentialBody(body);
  return serializeWrite(async () => {
    if (input.clear) {
      await credentials.unset(HINDSIGHT_CREDENTIAL_REF);
    } else {
      await credentials.set(HINDSIGHT_CREDENTIAL_REF, input.token);
      const config = await readHindsightConfig(path);
      if (Object.hasOwn(config, "apiToken")) {
        delete config.apiToken;
        await atomicWriteJson(path, config);
      }
    }
    const state = await readHindsightState(credentials, path);
    return { ...state, restartRequired: true };
  });
}

async function testHindsightConnection(credentials, body, path = hindsightConfigPath(), fetchImpl = fetch) {
  const config = body && Object.keys(body).length > 0
    ? assertHindsightConfigBody(body)
    : normalizeHindsightConfig(await readHindsightConfig(path));
  const stored = await credentials.resolve(HINDSIGHT_CREDENTIAL_REF);
  const raw = await readHindsightConfig(path);
  const token = stored?.value || (typeof raw.apiToken === "string" ? raw.apiToken : "");
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 8000);
  try {
    const response = await fetchImpl(`${validateHindsightUrl(config.serverMode, config.apiUrl)}/version`, {
      method: "GET",
      headers: token === "" ? { accept: "application/json" } : { accept: "application/json", authorization: `Bearer ${token}` },
      signal: controller.signal,
    });
    const text = await response.text();
    if (!response.ok) throw new Error(response.status === 401 || response.status === 403 ? "authentication-failed" : `http-${response.status}`);
    let version = text.trim();
    try {
      const parsed = JSON.parse(text);
      version = String(parsed.version || parsed.build || parsed.status || version);
    } catch {}
    return { ok: true, version: version || "available" };
  } catch (error) {
    if (error?.name === "AbortError") throw new Error("connection-timeout");
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

function profileBundles(profile) {
  const bundles = profile?.dsh?.profile?.bundles;
  if (!Array.isArray(bundles) || !bundles.every((item) => typeof item === "string")) {
    throw new Error("invalid-web-profile");
  }
  return bundles;
}

/** 按桌面托管顺序插回一个 bundle，同时保留所有未知用户 bundle 的相对位置。 */
function enableManagedBundle(bundles, packageName) {
  const next = bundles.filter((bundle) => bundle !== packageName);
  const targetOrder = MANAGED_PLUGINS.findIndex((item) => item.package === packageName);
  const later = new Set(MANAGED_PLUGINS.slice(targetOrder + 1).map((item) => item.package));
  const before = next.findIndex((bundle) => later.has(bundle));
  if (before >= 0) {
    next.splice(before, 0, packageName);
    return next;
  }
  const known = new Set([
    "dshmarket",
    CONTROL_BUNDLE,
    ...MANAGED_PLUGINS.slice(0, targetOrder).map((item) => item.package),
  ]);
  let after = -1;
  for (let index = 0; index < next.length; index += 1) {
    if (known.has(next[index])) after = index;
  }
  next.splice(after + 1, 0, packageName);
  return next;
}

function assertToggleBody(body) {
  const keys = Object.keys(body).sort().join(",");
  if (keys !== "enabled,package,profile" || body.profile !== "web") {
    throw new Error("invalid-request");
  }
  if (typeof body.package !== "string" || typeof body.enabled !== "boolean") {
    throw new Error("invalid-request");
  }
  if (PROTECTED_BUNDLES.has(body.package)) throw new Error("protected-bundle");
  if (!TOGGLEABLE.has(body.package)) throw new Error("unknown-managed-plugin");
}

async function readProfile(path = profilePath()) {
  const profile = JSON.parse(await readFile(path, "utf8"));
  profileBundles(profile);
  return profile;
}

/** 使用同目录临时文件替换 profile；失败时清理临时文件。 */
async function atomicWriteProfile(path, profile) {
  temporarySequence += 1;
  const temporary = join(dirname(path), `.package.json.${process.pid}.${temporarySequence}.tmp`);
  try {
    await writeFile(temporary, `${JSON.stringify(profile, null, 2)}\n`, "utf8");
    await rename(temporary, path);
  } catch (error) {
    await unlink(temporary).catch(() => {});
    throw error;
  }
}

function serializeWrite(operation) {
  const current = writeTail.then(operation, operation);
  writeTail = current.catch(() => {});
  return current;
}

async function listManagedPlugins(path = profilePath()) {
  const profile = await readProfile(path);
  const enabled = new Set(profileBundles(profile));
  return MANAGED_PLUGINS.map((item) => ({ ...item, enabled: enabled.has(item.package) }));
}

async function toggleManagedPlugin(body, path = profilePath()) {
  assertToggleBody(body);
  return serializeWrite(async () => {
    const profile = await readProfile(path);
    const originalBundles = profileBundles(profile);
    // 新版 Skin Center 已内置全部皮肤；切换时清理旧聚合载具，避免同一加载器重复注册。
    const bundles = originalBundles.filter((bundle) => !RETIRED_THEME_BUNDLES.has(bundle));
    const next = body.enabled
      ? enableManagedBundle(bundles, body.package)
      : bundles.filter((bundle) => bundle !== body.package);
    if (
      next.length !== originalBundles.length
      || next.some((bundle, index) => bundle !== originalBundles[index])
    ) {
      profile.dsh.profile.bundles = next;
      await atomicWriteProfile(path, profile);
    }
    return {
      ok: true,
      package: body.package,
      enabled: next.includes(body.package),
      restartRequired: true,
    };
  });
}

function makeRoutes() {
  return [
    {
      kind: "exact",
      path: `${API_PREFIX}/state`,
      handler: (req, res) => {
        if (!requireMethod(req, res, "GET") || !requireSameOrigin(req, res)) return;
        listManagedPlugins().then(
          (plugins) => json(res, 200, { ok: true, profile: "web", plugins }),
          (error) => json(res, 500, { ok: false, error: error.message }),
        );
      },
    },
    {
      kind: "exact",
      path: `${API_PREFIX}/toggle`,
      handler: (req, res) => {
        if (!requireMethod(req, res, "POST") || !requireSameOrigin(req, res)) return Promise.resolve();
        return readJsonBody(req).then(
          (body) => toggleManagedPlugin(body).then(
            (value) => json(res, 200, value),
            (error) => json(res, 400, { ok: false, error: error.message }),
          ),
          (error) => json(res, 400, { ok: false, error: error.message }),
        );
      },
    },
    {
      kind: "exact",
      path: `${HINDSIGHT_API_PREFIX}/state`,
      handler: (req, res, ctx) => {
        if (!requireMethod(req, res, "GET") || !requireSameOrigin(req, res)) return;
        readHindsightState(ctx.credentials).then(
          (value) => json(res, 200, value),
          (error) => json(res, 500, { ok: false, error: error.message }),
        );
      },
    },
    {
      kind: "exact",
      path: `${HINDSIGHT_API_PREFIX}/config`,
      handler: (req, res) => {
        if (!requireMethod(req, res, "POST") || !requireSameOrigin(req, res)) return Promise.resolve();
        return readJsonBody(req).then(
          (body) => saveHindsightConfig(body).then(
            (value) => json(res, 200, value),
            (error) => json(res, 400, { ok: false, error: error.message }),
          ),
          (error) => json(res, 400, { ok: false, error: error.message }),
        );
      },
    },
    {
      kind: "exact",
      path: `${HINDSIGHT_API_PREFIX}/credential`,
      handler: (req, res, ctx) => {
        if (!requireMethod(req, res, "POST") || !requireSameOrigin(req, res)) return Promise.resolve();
        return readJsonBody(req).then(
          (body) => updateHindsightCredential(ctx.credentials, body).then(
            (value) => json(res, 200, value),
            (error) => json(res, 400, { ok: false, error: error.message }),
          ),
          (error) => json(res, 400, { ok: false, error: error.message }),
        );
      },
    },
    {
      kind: "exact",
      path: `${HINDSIGHT_API_PREFIX}/test`,
      handler: (req, res, ctx) => {
        if (!requireMethod(req, res, "POST") || !requireSameOrigin(req, res)) return Promise.resolve();
        return readJsonBody(req).then(
          (body) => testHindsightConnection(ctx.credentials, body).then(
            (value) => json(res, 200, value),
            (error) => json(res, 400, { ok: false, error: error.message }),
          ),
          (error) => json(res, 400, { ok: false, error: error.message }),
        );
      },
    },
  ];
}

const inject = ["webServer", "credentials"];

/** 注册桌面托管插件状态与开关 API；路由失败不应拖垮核心 DSH。 */
function apply(ctx) {
  try {
    ctx.effect(() => {
      const disposers = makeRoutes().map((route) => ctx.webServer.register({
        ...route,
        handler: (req, res) => route.handler(req, res, ctx),
      }));
      return () => disposers.forEach((dispose) => dispose());
    }, "desktop-settings: managed plugin routes");
  } catch (error) {
    console.error("[desktop-settings] route registration failed:", error);
  }
}

export {
  API_PREFIX,
  CONTROL_BUNDLE,
  HINDSIGHT_API_PREFIX,
  HINDSIGHT_CLOUD_URL,
  HINDSIGHT_CREDENTIAL_REF,
  MANAGED_PLUGINS,
  PROTECTED_BUNDLES,
  apply,
  assertToggleBody,
  enableManagedBundle,
  inject,
  isSameOriginRequest,
  listManagedPlugins,
  makeRoutes,
  normalizeHindsightConfig,
  readHindsightState,
  saveHindsightConfig,
  testHindsightConnection,
  toggleManagedPlugin,
  updateHindsightCredential,
  validateHindsightUrl,
};
