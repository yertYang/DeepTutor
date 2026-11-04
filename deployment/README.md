# Docker server with updates from the Web UI

Use a dedicated Linux server checkout of the desired Git branch. The branch
must track its remote, and tracked files must be clean. The updater fetches that
upstream, permits only fast-forward merges, builds locally and replaces only the
`deeptutor` service. This deployment uses the fixed Compose project name `lorevia`.

Create `.env` in the checkout (keep existing port/data configuration there):

```dotenv
DEEPTUTOR_REPO=/srv/Lorevia
DEEPTUTOR_UPDATE_TOKEN=replace-with-a-random-token-at-least-32-characters-long
HOST_PORT_FRONTEND=3782
DEEPTUTOR_DATA_DIR=./data
```

Generate a token with `openssl rand -hex 32`. The repository must be mounted at
the same absolute path inside the updater as on the host. Configure remote Git
access on the server; public HTTPS remotes work without additional credentials.
Private SSH remotes need separately configured credentials in the updater.

From that checkout, initialize the deployment:

```sh
export DEEPTUTOR_BUILD_REVISION="$(git rev-parse HEAD)"
docker compose -f compose.yaml -f compose.server.yaml up -d --build
```

The default port binds to `127.0.0.1`; expose it through your HTTPS reverse proxy
or connect through an SSH tunnel. On first visit, set up the administrator account.
This override enables authentication; update operations require an administrator.
Back up an existing deployment before switching its Compose project/data path.

Open **Settings → About → Check now → Update**. Confirming calls the server API,
which delegates to the updater. The application keeps running during the build,
then restarts; the UI polls the persisted job across that restart. Settings and
data remain in the existing data mount. Commit revisions identify code updates
even when `VERSION` has not changed. Checks fetch the configured upstream branch,
including commits not associated with a published release.

If the build fails, the existing application container remains running. The
checkout may already have advanced, so fix the build problem and retry. A failed
health check after replacement is reported; automatic rollback is not provided.
An interrupted updater job is marked failed on its next start. Job details are
stored in the `update-state` volume. Do not run concurrent external deployments.

```sh
docker compose -f compose.yaml -f compose.server.yaml logs --tail=100 updater
```

The updater has the Docker socket and write access to this dedicated checkout.
It has no published port; keep it on a trusted server network. The main
application has neither the socket nor repository write access. Updating the
updater itself requires running the deployment command above again. The plain
`compose.yaml` and image-only Compose configuration do not enable UI updates.
