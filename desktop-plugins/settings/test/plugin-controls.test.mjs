import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import {
  assertToggleBody,
  isSameOriginRequest,
  listManagedPlugins,
  normalizeHindsightConfig,
  readHindsightState,
  saveHindsightConfig,
  testHindsightConnection,
  toggleManagedPlugin,
  updateHindsightCredential,
} from "../lib/index.js";

const directories = [];

async function fixture() {
  const directory = await mkdtemp(join(tmpdir(), "dsh-desktop-plugin-controls-"));
  directories.push(directory);
  const path = join(directory, "package.json");
  const profile = {
    dependencies: {
      "dsh-at-file": "link:C:/managed/dsh-at-file",
      "@changfenhuang/dsh-genui": "link:C:/managed/dsh-genui",
      "dsh-better-sidebar": "link:C:/managed/dsh-better-sidebar",
      "user-plugin": "1.2.3",
    },
    dsh: {
      profile: {
        bundles: [
          "@deepseek-ai/dsh-base",
          "@deepseek-ai/dsh-web-app",
          "dshmarket",
          "@dsh-desktop/settings",
          "dsh-at-file",
          "@changfenhuang/dsh-genui",
          "user-plugin",
        ],
      },
    },
  };
  await writeFile(path, JSON.stringify(profile), "utf8");
  return { path, profile };
}

afterEach(async () => {
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

test("设置客户端只注册预置插件与 Hindsight 页面", async () => {
  const client = await readFile(new URL("../lib/client.js", import.meta.url), "utf8");
  assert(client.includes('id: "desktop-managed-plugins"'));
  assert(client.includes('id: "desktop-memory"'));
  assert(!client.includes('id: "desktop-theme"'));
  assert(!client.includes('"web-ui.plugin.item"'));
  assert(!client.includes('"theme.nav"'));
});

test("只接受 web profile 白名单，并保护基础 bundle", () => {
  assert.throws(
    () => assertToggleBody({ profile: "web", package: "@dsh-desktop/runtime-services", enabled: false }),
    /protected-bundle/,
  );
  assert.throws(
    () => assertToggleBody({ profile: "web", package: "dshmarket", enabled: false }),
    /protected-bundle/,
  );
  assert.throws(
    () => assertToggleBody({ profile: "web", package: "unknown-plugin", enabled: false }),
    /unknown-managed-plugin/,
  );
  assert.throws(
    () => assertToggleBody({ profile: "other", package: "@changfenhuang/dsh-genui", enabled: false }),
    /invalid-request/,
  );
  assert.throws(
    () => assertToggleBody({ profile: "web", package: "dsh-at-file", enabled: false }),
    /unknown-managed-plugin/,
  );
  assert.throws(
    () => assertToggleBody({ profile: "web", package: "@liustack/modlens", enabled: false }),
    /unknown-managed-plugin/,
  );
});

function fakeCredentials(initial = undefined) {
  let value = initial;
  return {
    async describe() {
      return { configured: typeof value === "string", source: value === undefined ? undefined : "file", writable: true };
    },
    async resolve() {
      return value === undefined ? undefined : { value, source: "file" };
    },
    async set(_ref, next) {
      value = next;
    },
    async unset() {
      value = undefined;
    },
  };
}

test("API 只接受回环 Host 与同源浏览器请求", () => {
  assert.equal(
    isSameOriginRequest({
      headers: {
        host: "127.0.0.1:3210",
        origin: "http://127.0.0.1:3210",
        "sec-fetch-site": "same-origin",
      },
    }),
    true,
  );
  assert.equal(
    isSameOriginRequest({ headers: { host: "192.168.1.10:3210" } }),
    false,
  );
  assert.equal(
    isSameOriginRequest({
      headers: {
        host: "localhost:3210",
        origin: "https://attacker.example",
        "sec-fetch-site": "cross-site",
      },
    }),
    false,
  );
});

test("开关只修改 bundles，并保留历史 At File 依赖与未知用户 bundle", async () => {
  const { path, profile } = await fixture();
  await toggleManagedPlugin(
    { profile: "web", package: "@changfenhuang/dsh-genui", enabled: false },
    path,
  );
  const updated = JSON.parse(await readFile(path, "utf8"));
  assert.deepEqual(updated.dependencies, profile.dependencies);
  assert(updated.dsh.profile.bundles.includes("user-plugin"));
  assert(updated.dsh.profile.bundles.includes("dsh-at-file"));
  assert(!updated.dsh.profile.bundles.includes("@changfenhuang/dsh-genui"));

  await toggleManagedPlugin(
    { profile: "web", package: "@changfenhuang/dsh-genui", enabled: false },
    path,
  );
  const repeated = JSON.parse(await readFile(path, "utf8"));
  assert.deepEqual(repeated, updated);
});

test("启用独立 Skin Center 时移除退役主题载具", async () => {
  const { path } = await fixture();
  const profile = JSON.parse(await readFile(path, "utf8"));
  profile.dependencies["@linxin666/dsh-skins"] = "link:C:/managed/dsh-skins";
  profile.dependencies["@linxin666/dsh-client-ui-skin-center"] = "link:C:/managed/skin-center";
  profile.dsh.profile.bundles.push("@linxin666/dsh-skins");
  await writeFile(path, JSON.stringify(profile), "utf8");

  await toggleManagedPlugin(
    { profile: "web", package: "@linxin666/dsh-client-ui-skin-center", enabled: true },
    path,
  );

  const updated = JSON.parse(await readFile(path, "utf8"));
  assert(updated.dsh.profile.bundles.includes("@linxin666/dsh-client-ui-skin-center"));
  assert(!updated.dsh.profile.bundles.includes("@linxin666/dsh-skins"));
  assert.equal(updated.dependencies["@linxin666/dsh-skins"], "link:C:/managed/dsh-skins");
  assert.equal(updated.dependencies["@linxin666/dsh-client-ui-skin-center"], "link:C:/managed/skin-center");
});

test("并发开关串行合并，列表返回最终状态", async () => {
  const { path } = await fixture();
  await Promise.all([
    toggleManagedPlugin(
      { profile: "web", package: "dsh-better-sidebar", enabled: false },
      path,
    ),
    toggleManagedPlugin(
      { profile: "web", package: "@changfenhuang/dsh-genui", enabled: false },
      path,
    ),
  ]);
  const rows = await listManagedPlugins(path);
  assert.equal(rows.some((row) => row.package === "dsh-at-file"), false);
  assert.equal(rows.some((row) => row.package === "@liustack/modlens"), false);
  assert.equal(rows.find((row) => row.package === "dsh-better-sidebar").enabled, false);
  assert.equal(rows.find((row) => row.package === "@changfenhuang/dsh-genui").enabled, false);
});

test("Hindsight 保存只修改托管字段并强制项目显式启用", async () => {
  const directory = await mkdtemp(join(tmpdir(), "dsh-desktop-hindsight-"));
  directories.push(directory);
  const path = join(directory, "coding-agent.json");
  await writeFile(path, JSON.stringify({
    customRoot: { keep: true },
    serverMode: "daemon",
    apiUrl: "http://old.test",
    harnesses: { dsh: { customDsh: 7, optInOnly: false }, codex: { disabled: true } },
  }), "utf8");

  const result = await saveHindsightConfig({
    serverMode: "self-hosted",
    apiUrl: "http://127.0.0.1:8888/",
    optInPaths: [directory, directory],
  }, path);
  const updated = JSON.parse(await readFile(path, "utf8"));
  assert.equal(result.restartRequired, true);
  assert.deepEqual(updated.customRoot, { keep: true });
  assert.deepEqual(updated.harnesses.codex, { disabled: true });
  assert.equal(updated.harnesses.dsh.customDsh, 7);
  assert.equal(updated.harnesses.dsh.optInOnly, true);
  assert.deepEqual(updated.harnesses.dsh.optInPaths, [directory]);
  assert.equal(updated.apiUrl, "http://127.0.0.1:8888");
});

test("Hindsight 凭据不回显，新密钥成功写入后移除旧明文", async () => {
  const directory = await mkdtemp(join(tmpdir(), "dsh-desktop-hindsight-secret-"));
  directories.push(directory);
  const path = join(directory, "coding-agent.json");
  await writeFile(path, JSON.stringify({ apiToken: "legacy-secret", unknown: true }), "utf8");
  const credentials = fakeCredentials();

  const before = await readHindsightState(credentials, path);
  assert.equal(before.credential.configured, true);
  assert.equal(before.credential.source, "legacy-file");
  assert(!JSON.stringify(before).includes("legacy-secret"));

  const after = await updateHindsightCredential(credentials, { token: "new-secret" }, path);
  assert.equal(after.credential.configured, true);
  assert(!JSON.stringify(after).includes("new-secret"));
  const config = JSON.parse(await readFile(path, "utf8"));
  assert.equal(config.apiToken, undefined);
  assert.equal(config.unknown, true);
});

test("Hindsight 拒绝非法 URL 和相对项目路径", async () => {
  const directory = await mkdtemp(join(tmpdir(), "dsh-desktop-hindsight-invalid-"));
  directories.push(directory);
  const path = join(directory, "coding-agent.json");
  await assert.rejects(
    saveHindsightConfig({ serverMode: "self-hosted", apiUrl: "file:///tmp", optInPaths: [] }, path),
    /invalid-api-url/,
  );
  await assert.rejects(
    saveHindsightConfig({ serverMode: "cloud", apiUrl: "", optInPaths: ["relative"] }, path),
    /invalid-opt-in-path/,
  );
  assert.deepEqual(normalizeHindsightConfig({}), {
    serverMode: "cloud",
    apiUrl: "https://api.hindsight.vectorize.io",
    optInPaths: [],
  });
});

test("Hindsight 连接测试使用凭据且只返回版本", async () => {
  const directory = await mkdtemp(join(tmpdir(), "dsh-desktop-hindsight-test-"));
  directories.push(directory);
  const path = join(directory, "coding-agent.json");
  const credentials = fakeCredentials("private-token");
  let authorization = "";
  const result = await testHindsightConnection(
    credentials,
    { serverMode: "self-hosted", apiUrl: "https://memory.example", optInPaths: [] },
    path,
    async (_url, options) => {
      authorization = options.headers.authorization;
      return new Response(JSON.stringify({ version: "1.2.3" }), { status: 200 });
    },
  );
  assert.equal(authorization, "Bearer private-token");
  assert.deepEqual(result, { ok: true, version: "1.2.3" });
  assert(!JSON.stringify(result).includes("private-token"));
});
