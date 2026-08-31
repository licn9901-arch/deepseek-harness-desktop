# Third-Party Notices

DeepSeek Harness Desktop is a community project and is not an official DeepSeek product.

The preview installer bundles the following third-party software:

| Component | Pinned version | License / source |
|---|---:|---|
| Node.js | 22.22.3 | Node.js license, <https://github.com/nodejs/node> |
| `@deepseek-ai/dsh` | 0.1.2-alpha.2 | MIT, <https://www.npmjs.com/package/@deepseek-ai/dsh> |
| `dshmarket` | 1.38.1 | MIT, <https://github.com/dsh-market/dsh-market> |
| `pnpm` | 10.34.5 | MIT, <https://github.com/pnpm/pnpm> |
| `@changfenhuang/dsh-genui` | 0.9.6 | MIT, <https://github.com/omdsh-dev/dsh-genui> |
| `dsh-better-sidebar` | 0.17.1 | MIT, <https://github.com/omdsh-dev/DSH-better-sidebar> |
| `@linxin666/dsh-client-ui-skin-center` | 0.3.10 | Apache-2.0, <https://github.com/zhu1090093659/dsh-web-ui> |
| `@vectorize-io/hindsight-coding-agents` | 0.4.3 | MIT, <https://github.com/vectorize-io/hindsight/tree/main/hindsight-integrations/coding-agents> |
| `@cubee-slide/skills-mcp-manager` | 0.2.4 | MIT, <https://github.com/licn9901-arch/dsh-skills-mcp-manager> |
| Tauri | 2.x | Apache-2.0 OR MIT, <https://github.com/tauri-apps/tauri> |
| esbuild | 0.28.2 | MIT, <https://github.com/evanw/esbuild> |
| `zip` | 8.6.0 | MIT, <https://github.com/zip-rs/zip2> |
| `fs2` | 0.4.3 | Apache-2.0 OR MIT, <https://github.com/danburkert/fs2-rs> |
| `windows-sys` | 0.61.2 | Apache-2.0 OR MIT, <https://github.com/microsoft/windows-rs> |

The build pipeline copies the licenses shipped with the pinned Node.js archive and npm dependency tree into the packaged runtime. Those upstream license texts govern their respective components.

The packaged Market keeps its upstream `dshmarket` package and client registration identity. Desktop Runtime Services always use the bundled pnpm 10.34.5 and perform one transactional dependency-tree rebuild only when pnpm explicitly reports incompatible modules or hoist metadata.

The managed plugins can access capabilities exposed by the DSH Host. In particular, Better Sidebar can read and write workspace files, invoke Git, and create local PTY processes; Skin Center can read user-selected local wallpaper directories and writes the active selection under `$DSH_HOME`; GenUI actions send user interaction data back to the active model; Hindsight can send opted-in project memory to the endpoint configured by the user; Skills/MCP Manager can delete Skills and stores MCP `env` and `headers` in plaintext at `~/.dsh/mcp.json`. DSH Desktop disables install scripts for all managed npm plugins, defaults Hindsight to an empty opt-in list, and initializes Better Sidebar HTTP/HTTPS interception to off for a first managed installation. Plugins installed through DSH Market run with the same host permissions as the desktop application and are not protected by package signatures, permission manifests, or a process sandbox.
