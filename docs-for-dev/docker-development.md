# Docker 开发

在项目根目录执行（Docker Compose 2.32+，当前 Docker Desktop 支持）：

```sh
docker compose -f compose.dev.yaml up --build --watch
```

打开 <http://localhost:3783>。默认端口避开旧本机服务使用的 3782。
首次需要下载 Linux 依赖并编译 Rust，后续复用命名卷缓存。
前端修改走 Next.js HMR；Rust 源码和依赖清单变更会重启容器并重新编译。
后端只监听容器内的回环地址，前端通过同容器代理访问它。

也可以后台运行：

```sh
docker compose -f compose.dev.yaml up -d --build
docker compose -f compose.dev.yaml logs -f --tail=100 dev
```

后台启动不自动监听 Rust 变更，修改后执行
`docker compose -f compose.dev.yaml restart dev`；前端仍会自动热更新。
不要同时启动两种方式的开发服务。

默认限制 4 GB 内存、2 CPU，Node old-space 为 2048 MB，Rust 并发 2，
禁用 Rust 调试符号以降低构建开销。初次 Rust 编译在前端启动前完成，减少内存峰值。
Docker Desktop 虚拟机本身另有开销，这不是整个 Docker 应用的 4 GB 总上限。
Docker Desktop 的全局内存额度应能容纳该容器及其他在用容器。

默认使用 Webpack 低内存模式。选择 Turbopack 或调整预算：

```sh
DEV_BUNDLER=turbopack docker compose -f compose.dev.yaml up --build --watch
DEV_MEMORY_LIMIT=5g DEV_HEAP_MB=2560 DEV_CPUS=2 docker compose -f compose.dev.yaml up --build --watch
```

可用变量：`DEV_PORT`（3783）、`DEV_MEMORY_LIMIT`（4g）、`DEV_CPUS`（2）、
`DEV_HEAP_MB`（2048）、`DEV_TURBO_MEMORY_MB`（768）、`DEV_BUNDLER`（webpack）。
可在根目录未跟踪的 `.env` 中保存这些变量。

源码挂载到 `/workspace`。`web/node_modules`、浏览器工具依赖、Next 缓存、Rust target、
Cargo 下载缓存和 npm 缓存使用独立 Linux 命名卷，不复用 macOS 编译产物。
依赖清单的指纹变化会触发 `npm ci`，无需手动删除依赖卷。
Next.js 会生成类型声明文件，不要同时在同一份源码上启动本机 Next 开发服务。

开发数据使用独立 `dev-data` 卷，初始为空；通过页面配置开发账号与模型。
本机已有数据不自动导入。模型地址若指向宿主机服务，使用 `host.docker.internal`
替代 `localhost`。Linux 环境不提供 macOS Vision OCR，系统能力可能有差异。
浏览器测试工具的 npm 依赖会安装；需要真实浏览器时，在容器内为 Playwright
安装 Chromium 及系统依赖：

```sh
docker compose -f compose.dev.yaml exec -w /workspace/packages/browser-testing dev npx playwright install --with-deps chromium
```

这项可选安装位于当前容器，重新创建容器后需要重装。

检查和停止：

```sh
docker compose -f compose.dev.yaml exec dev node --test web/scripts/dev-memory.test.mjs
docker compose -f compose.dev.yaml exec dev cargo test --locked -p deeptutor-server
docker compose -f compose.dev.yaml exec -w /workspace/web dev npx tsc --noEmit
docker compose -f compose.dev.yaml down
```

`down` 保留数据和依赖缓存。不要使用 `down -v` 清理常规开发缓存，因为它会同时删除开发数据。
Docker 引擎出现 EOF 或无响应时，先恢复 Docker Desktop，再重试启动；不回退到宿主机编译。
