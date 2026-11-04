# 项目开发约定

## 默认在 Docker 中操作

- 开发环境使用根目录 `compose.dev.yaml`，服务名 `dev`，容器工作目录 `/workspace`。
- **项目启动、依赖安装、编译、测试、类型检查及代码生成默认全部在开发容器内执行。**
  不要在宿主机运行 `npm install/ci/run`、`npx next`、`cargo build/test/check/run`，
  也不要重新启动本机 Next.js / Rust 服务。用户明确要求本机运行时才例外。
- 宿主机可以编辑源码、查看文件、执行 Git，以及运行 Docker / Compose 命令。
- Docker 不可用时，报告具体错误并处理引擎问题，不要自动回退到本机运行。
- 不要使用生产 `compose.yaml` 或旧的 `docker-compose.dev.yml` 作为热更新开发环境。

## 常用命令

```sh
# 后台启动（首次安装 Linux 依赖、编译后端可能较慢）
docker compose -f compose.dev.yaml up -d --build

# 查看状态和日志
docker compose -f compose.dev.yaml ps
docker compose -f compose.dev.yaml logs --tail=100 dev

# 前端自动热更新；Rust 修改后重启会自动增量编译
docker compose -f compose.dev.yaml restart dev

# 需要自动监听 Rust 修改时，终端前台运行
docker compose -f compose.dev.yaml up --build --watch

# 在容器内执行验证
docker compose -f compose.dev.yaml exec dev node --test web/scripts/dev-memory.test.mjs
docker compose -f compose.dev.yaml exec dev cargo test --locked -p deeptutor-server
docker compose -f compose.dev.yaml exec -w /workspace/web dev npx tsc --noEmit

# 进入容器 / 停止（保留开发数据及缓存卷）
docker compose -f compose.dev.yaml exec dev bash
docker compose -f compose.dev.yaml down
```

- 默认访问 `http://localhost:3783`；后端仅在同容器 `127.0.0.1:8002` 监听。
- 源码通过绑定挂载同步，修改文件后无需复制源码进镜像。
- 首次启动前后端会顺序准备，健康检查就绪前先查看日志，不要反复重建镜像。
- 修改 lockfile 后重启服务即可重新安装依赖。修改工具链镜像才需要 `--build`。
- 不要把 macOS `node_modules` 或 `target` 复制进容器；依赖、Next 和 Cargo 缓存均使用命名卷。

## 内存与数据

- 默认容器限制为 4 GB 内存、2 CPU，不使用额外交换空间；Rust 编译并发为 2。
- 默认使用 Webpack 低内存开发模式并保留 HMR。V8 old-space 默认预算为 2048 MB。
  可选 Turbopack 的原生缓存目标为 768 MB，不能仅用 Node 堆上限代替它。
- 不要同时启动本机与容器开发服务，或并行执行多个大型构建。
- 调整预算时显式使用 `DEV_MEMORY_LIMIT`、`DEV_HEAP_MB`、`DEV_CPUS`，
  不要悄悄取消容器内存限制。
- 开发数据保存在独立 `dev-data` 命名卷，不默认挂载、迁移或修改现有本机业务数据。
- 未经用户明确授权，不执行 `down -v`、`docker volume prune`、`docker system prune`，
  不删除 `data/`、`native-home/`、发布包或其他项目的缓存。
- 详细说明见 `docs-for-dev/docker-development.md` 和 `docs-for-dev/development-memory.md`。

## 改动范围

- 工作区可能有其他未提交改动；只修改当前任务需要的文件，不覆盖或还原无关工作。
- 清理缓存前检查对应开发或编译进程是否已经停止。
- 验证结果中区分配置检查通过和容器实际运行通过，不能将 Docker 引擎故障描述为验证成功。
