import { readFile, readdir, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { isAbsolute, join, relative, resolve } from "node:path";

const MAX_WIDTH = 480;
const MAX_FILE_BYTES = 384 * 1024;
const MAX_TOTAL_BYTES = 4 * 1024 * 1024;
const SKIN_CENTER = "@linxin666/dsh-client-ui-skin-center";

/** 读取命令行目录参数，并拒绝相对路径以避免处理范围漂移。 */
function readDirectoryArgument(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : null;
  if (!value || !isAbsolute(value)) {
    throw new Error(`${name} must be an absolute directory path.`);
  }
  return resolve(value);
}

/** 递归收集内置皮肤画廊支持的 PNG 或 JPEG 预览图。 */
async function collectPreviewFiles(root) {
  const files = [];
  for (const skin of await readdir(root, { withFileTypes: true })) {
    if (!skin.isDirectory()) continue;
    const previewRoot = join(root, skin.name, "preview");
    let entries;
    try {
      entries = await readdir(previewRoot, { withFileTypes: true });
    } catch (error) {
      if (error.code === "ENOENT") continue;
      throw error;
    }
    for (const entry of entries) {
      if (entry.isFile() && /\.(png|jpe?g)$/i.test(entry.name)) {
        files.push(join(previewRoot, entry.name));
      }
    }
  }
  return files.sort();
}

/** 将设置页预览缩放为稳定的小图，保留主题运行时使用的原始背景资产。 */
async function optimizePreview(sharp, file) {
  const before = (await stat(file)).size;
  const input = await readFile(file);
  const image = sharp(input).resize({ width: MAX_WIDTH, withoutEnlargement: true });
  const output = /\.png$/i.test(file)
    ? await image.png({ compressionLevel: 9, adaptiveFiltering: true, palette: true, quality: 80 }).toBuffer()
    : await image.jpeg({ quality: 80, progressive: true, mozjpeg: true }).toBuffer();
  if (output.length > MAX_FILE_BYTES) {
    throw new Error(`Optimized preview exceeds ${MAX_FILE_BYTES} bytes: ${file}`);
  }
  await writeFile(file, output);
  return { before, after: output.length };
}

const hostNodeModules = readDirectoryArgument("--host-node-modules");
const pluginRoot = readDirectoryArgument("--plugin-root");
const skinsRoot = join(pluginRoot, "node_modules", SKIN_CENTER, "skins");
const escaped = relative(pluginRoot, skinsRoot);
if (escaped.startsWith("..") || isAbsolute(escaped)) {
  throw new Error(`Skin preview root escaped plugin staging: ${skinsRoot}`);
}

const requireFromHost = createRequire(join(hostNodeModules, "package.json"));
const sharp = requireFromHost("sharp");
const files = await collectPreviewFiles(skinsRoot);
if (files.length === 0) throw new Error(`No Skin Center previews found under ${skinsRoot}`);

let beforeBytes = 0;
let afterBytes = 0;
for (const file of files) {
  const result = await optimizePreview(sharp, file);
  beforeBytes += result.before;
  afterBytes += result.after;
}
if (afterBytes > MAX_TOTAL_BYTES) {
  throw new Error(`Optimized previews exceed ${MAX_TOTAL_BYTES} bytes: ${afterBytes}`);
}
console.log(
  `Skin previews optimized: ${files.length} files, ${beforeBytes} -> ${afterBytes} bytes`,
);
