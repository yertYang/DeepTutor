# Fork deployment

The `deploy/zorvya` branch in `yertYang/DeepTutor` combines the deployed fixes
for Redis event JSON, historical attachments, bulk model import and discovery,
Codex custom headers, video narration dependencies, and conversation reasoning.

`Dockerfile.deploy` builds the frontend and copies the fork's complete Python
application into the pinned official v1.7.1 runtime. It installs `ffmpeg` and
`edge-tts` and runs the focused frontend regression tests before building.
Dependency or base-version changes also require updating this build definition.

```bash
git clone --branch deploy/zorvya https://github.com/yertYang/DeepTutor.git
cd DeepTutor
docker build -f Dockerfile.deploy --build-arg VCS_REF="$(git rev-parse HEAD)" -t deeptutor:fork-deploy .
```

Preserve the existing `/app/data` and workspace mounts when replacing the
application container. Keep runtime credentials and model configuration outside
the repository. The image labels identify the fork and exact source commit.

For subsequent source updates, pull `origin deploy/zorvya` with `--ff-only`,
rebuild this Dockerfile with the current commit as `VCS_REF`, and recreate the
application container. The upstream repository remains available as a separate
remote for reviewing and integrating future changes.
