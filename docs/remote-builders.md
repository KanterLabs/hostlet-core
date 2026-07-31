# Remote builders

Hostlet can build application images on separate, outbound-only Linux VMs and
release the immutable OCI digest on the app runner. This keeps compiler,
BuildKit, and dependency-install memory away from hosted applications.

## Architecture

1. A deployment is pinned to an exact Git commit and queued in a build pool.
2. The selected builder resolves the complete runtime, builds or mirrors every
   service image, and publishes one immutable OCI release index.
3. The API validates and stores the versioned artifact manifest, removes push
   credentials from the completed job, and queues a release on the app runner.
4. The runner logs in with a read-only identity, pulls every service and its
   release bundle by `sha256` digest, then performs health checks and atomic
   routing activation without a Git checkout or any build command.

The same build, publish, and release phases are used by the local pool and VM
pools. Dockerfile, generated/Railpack, repository Compose, managed add-on
Compose, and inferred frontend/backend deployments therefore have no
pool-specific fallback. If a selected VM pool has no online compatible builder,
the deployment waits visibly for that pool and never consumes app-server RAM.

Build failures never replace the current working deployment. Builder agents do
not receive app-routing capabilities and require no inbound ports.

## Bundled disposable registry

Core includes a small zot registry in both Compose stacks. `hostlet init`
generates separate bcrypt-authenticated identities:

- `hostlet-builder`: read/create/update/delete under `hostlet/apps/**`.
- `hostlet-runner`: read-only under `hostlet/apps/**`.

Compose can render before initialization, but empty registry passwords are not
usable: deployment enqueue fails closed until `hostlet init` has generated the
credentials and matching htpasswd file.

The registry stores data in the `hostlet-registry` Docker volume. It is
deliberately excluded from Hostlet backups: Git is the source of truth and a
missing artifact can be rebuilt. Per repository, zot retains the three newest
deployment tags plus tags pushed in the last seven days; garbage collection
runs hourly after retired manifests become eligible.

Local builds use `HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL` when it is configured,
normally the host-network agent's loopback URL. The API uses the separate
`HOSTLET_ARTIFACT_REGISTRY_INTERNAL_URL` for container-network status checks.
This avoids a public-proxy round trip without leaking Compose-only DNS names to
the agent. The default public URL, `http://127.0.0.1:5000`, supports the local
runner only. Before enabling a remote pool, publish the registry through a
trusted HTTPS reverse proxy and set `HOSTLET_ARTIFACT_REGISTRY_URL` to that
builder-reachable origin. Do not expose plain HTTP or the raw port to the
Internet.

## Add a VM builder

In **Settings → Build fleet**, create a VM pool and click **Copy install
command**. Run the command on a clean Linux VM with Docker Engine, Buildx,
systemd, and outbound HTTPS access. The enrollment token is single-use and
expires after 15 minutes. The installer accepts amd64 and arm64 builders and
runs the pinned Hostlet agent image with only the Docker socket and a dedicated
state directory mounted. Published Core release images are currently amd64;
arm64 builders must supply a pinned arm64 agent image with `--image`.

Drain a builder before maintenance. Revoking it invalidates its agent token;
leased work is recovered by the normal job lease and retry mechanism.

## Cloudflare qualification

Cloudflare remains an experimental provider. As of July 2026, Workers Builds
offers attractive included minutes and 8 GB build memory, but its API builds and
deploys Worker projects. Cloudflare Containers can run amd64 images and scale to
zero, while Wrangler builds those images using local Docker or CI. Neither
surface currently supplies Hostlet's generic privileged Docker/BuildKit job,
caller-selected OCI push, streaming log, and cancellation contract.

Cloudflare pools therefore fail closed during qualification and cannot become
the default. Re-evaluate when Cloudflare exposes those primitives or when a
Hostlet dispatcher built on the Sandbox SDK can prove the same conformance suite
as a VM builder.

## Hostlet Cloud overlay handoff

The scheduler, agent protocol, migrations, build execution helper, and fleet API
module are shared Core files. After advancing Cloud's Core submodule, Cloud must
mirror the new route registrations from Core's `apps/api/src/lib.rs` because
Cloud replaces that file wholesale. Cloud must also supply the six artifact
registry/agent image environment variables to its API deployment. Cloud replaces
Core's `apps/web/app` tree, so its Settings and app-detail screens need their own
thin UI over the inherited fleet endpoints.

Do not point Cloud at Core's loopback registry default. Publish zot or another
OCI Distribution-compatible registry through HTTPS, keep the runner credential
read-only, and leave Cloudflare pools non-default until qualification passes.

## Artifact manifest v2

The builder result records the build/deployment/app IDs, exact commit, target
platform, build-plan digest, deployment OCI index, release bundle, and every
digest-qualified service image. All images—including Compose images such as
Postgres and Redis—are mirrored into the app's Hostlet namespace. The API
rejects manifests whose assignment, platform, service set, or registry
namespace does not match the queued build.

The OCI index has the single retained `deployment-<id>` tag and references the
bundle plus every service manifest, so registry retention removes deployments
atomically. The release bundle contains no app secret values; the signed runner
job supplies current runtime environment values separately.
