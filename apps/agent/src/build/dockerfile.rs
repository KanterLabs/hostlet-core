//! Dockerfile generation for auto-packaged Node apps.
//!
//! Two runtime shapes are produced from a shared deps/builder foundation:
//! a static-site image served by `serve`, and a long-running Node server image.

use super::{Framework, PackageManager};

/// Sentinel start command meaning "serve the built static output" rather than
/// running a Node process. Threaded from [`super::pick_start_command`] and kept
/// byte-identical because callers (and tests) match on this exact string.
pub(crate) const STATIC_START_SENTINEL: &str = "__hostlet_static";

/// Base image used for every build stage.
const NODE_IMAGE: &str = "node:22-alpine";
/// Pinned pnpm version, activated through corepack.
const PNPM_VERSION: &str = "10.33.2";
/// Pinned `serve` version used to host static builds.
const SERVE_VERSION: &str = "14.2.5";

impl PackageManager {
    pub(super) fn install_command(self) -> String {
        match self {
            Self::Npm => "npm ci".to_string(),
            Self::Pnpm => format!(
                "corepack enable && corepack prepare pnpm@{PNPM_VERSION} --activate && pnpm install --frozen-lockfile --config.dangerouslyAllowAllBuilds=true"
            ),
            Self::Yarn => "corepack enable && yarn install --frozen-lockfile".to_string(),
        }
    }

    /// Command that strips dev dependencies after the build stage.
    fn prune_line(self) -> String {
        match self {
            Self::Npm => "RUN npm prune --omit=dev\n".to_string(),
            Self::Pnpm => format!(
                "RUN corepack enable && corepack prepare pnpm@{PNPM_VERSION} --activate && pnpm prune --prod\n"
            ),
            Self::Yarn => {
                "RUN corepack enable && yarn install --production --ignore-scripts --prefer-offline\n"
                    .to_string()
            }
        }
    }
}

/// The `FROM ... AS deps` + `FROM ... AS builder` stages shared by both runtime
/// shapes. `build_command` is rendered as a `RUN` line when present.
fn deps_and_builder_stages(install: &str, build_command: Option<&str>) -> String {
    let build_line = build_command
        .map(|command| format!("RUN {command}\n"))
        .unwrap_or_default();
    format!(
        "FROM {NODE_IMAGE} AS deps\n\
         WORKDIR /app\n\
         COPY package.json package-lock.json* pnpm-lock.yaml* yarn.lock* ./\n\
         RUN {install}\n\
         \n\
         FROM {NODE_IMAGE} AS builder\n\
         WORKDIR /app\n\
         COPY --from=deps /app/node_modules ./node_modules\n\
         COPY . .\n\
         {build_line}"
    )
}

/// Renders the complete Dockerfile for an auto-packaged Node app.
///
/// `start_command` is either [`STATIC_START_SENTINEL`] (produce a static-serve
/// image) or a literal command to run as the container entrypoint.
pub(crate) fn generated_node_dockerfile(
    pm: PackageManager,
    install_command: Option<&str>,
    build_command: Option<&str>,
    start_command: &str,
    port: i64,
    framework: Framework,
) -> String {
    let install = install_command
        .map(str::to_string)
        .unwrap_or_else(|| pm.install_command());
    let foundation = deps_and_builder_stages(&install, build_command);

    if start_command == STATIC_START_SENTINEL {
        return format!(
            "{foundation}\
             \n\
             FROM {NODE_IMAGE} AS runner\n\
             WORKDIR /app\n\
             RUN npm install -g serve@{SERVE_VERSION} && addgroup -S hostlet && adduser -S hostlet -G hostlet\n\
             COPY --from=builder --chown=hostlet:hostlet /app/dist ./dist\n\
             USER hostlet\n\
             ENV NODE_ENV=production\n\
             ENV PORT={port}\n\
             EXPOSE {port}\n\
             CMD [\"sh\", \"-lc\", \"serve -s dist -l tcp://0.0.0.0:${{PORT}}\"]\n"
        );
    }

    let runner_copy = match framework {
        Framework::Next => {
            "COPY --from=builder --chown=hostlet:hostlet /app/.next ./.next\n\
             COPY --from=builder --chown=hostlet:hostlet /app/public ./public\n"
        }
        Framework::Nuxt => "COPY --from=builder --chown=hostlet:hostlet /app/.output ./.output\n",
        _ => "COPY --from=builder --chown=hostlet:hostlet /app .\n",
    };
    let effective_start = if framework == Framework::Nuxt && start_command == "npm run start" {
        "node .output/server/index.mjs"
    } else {
        start_command
    };
    let start_line = format!(
        "CMD [\"sh\", \"-lc\", {}]",
        serde_json::to_string(effective_start).expect("string serialization cannot fail")
    );
    format!(
        "{foundation}\
         RUN mkdir -p public\n\
         {prune_line}\
         \n\
         FROM {NODE_IMAGE} AS runner\n\
         WORKDIR /app\n\
         RUN addgroup -S hostlet && adduser -S hostlet -G hostlet\n\
         COPY --from=builder --chown=hostlet:hostlet /app/package.json ./package.json\n\
         COPY --from=builder --chown=hostlet:hostlet /app/node_modules ./node_modules\n\
         {runner_copy}\
         USER hostlet\n\
         ENV NODE_ENV=production\n\
         ENV NPM_CONFIG_CACHE=/tmp/.npm\n\
         ENV PORT={port}\n\
         EXPOSE {port}\n\
         {start_line}\n",
        prune_line = pm.prune_line(),
    )
}
