# 本地开发内存

项目现已默认在 Docker 中开发，按 `docker-development.md` 和根目录 `AGENTS.md` 操作。
以下本机命令仅供用户明确选择本机开发时参考。

Next.js 16 的 Turbopack 使用 Rust 原生缓存，不能只靠 Node 的堆上限控制。
`web/scripts/dev.mjs` 默认将 V8 old-space 上限设为 2048 MB，开发配置将
Turbopack 内存回收目标设为 1024 MB；小内存机器和有 cgroup 限制的容器会按比例缩小。
这两个数不是整个服务的 RSS 硬上限，线程、Buffer、子进程等还需要额外内存。
Turbopack 的目标也允许编译时短暂超出。

正常开发（Turbopack，保留热更新）：

```sh
cd web
npm run dev -- --port 3782
```

内存紧张时使用 Webpack 低内存模式，保留热更新，但编译可能更慢。
该模式降低模块编译并发，减少非活动页面保留，允许回收磁盘缓存的内存副本：

```sh
cd web
npm run dev:low-memory -- --port 3782
```

先在原终端 Ctrl+C 停止旧服务，再启动新模式；同一份开发缓存不要同时启动两个服务。
这些命令只启动前端，后端按原来的方式启动。

需要更多编译空间时可以调整（单位 MB）：

```sh
DEEPTUTOR_DEV_HEAP_MB=3072 DEEPTUTOR_TURBO_MEMORY_MB=1536 npm run dev -- --port 3782
```

显式设置的 `NODE_OPTIONS=--max-old-space-size=...` 优先于脚本的堆预算。
使用 `npm run dev` 系列入口才能将预算传递给 Next 的渲染子进程；
直接运行 `npx next dev` 会绕过 Node 堆预算。
开发预算不改变生产构建配置。

同时运行前端生产构建、Rust release 编译和开发服务器会叠加峰值。
内存紧张时顺序执行构建；Rust 可用 `CARGO_BUILD_JOBS=2 cargo build ...` 降低并行编译数。
只调后端或验收功能时，使用已经构建好的 standalone 前端启动项目，可省去开发编译缓存，
但前端修改需要重新构建才能生效。

验证内存预算：`node --test web/scripts/dev-memory.test.mjs`。
