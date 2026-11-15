//! `docker`: a container image for the project, and `docker-kubernetes`, a
//! minimal deployment of it.
//!
//! A module rather than a file every project gets, because the image is one
//! way to ship among several, and a project that deploys some other way
//! should not carry a Dockerfile nobody builds.
//!
//! The Dockerfile's text depends on the project name and on one module: a
//! serve-only binary ignores the `serve` argument, and the config and
//! migrations directories are made empty when there are none. The health
//! check is the exception. With `observability-health`, `/up` must answer
//! 2xx (`curl -f`). Without it, any HTTP answer passes, so the 404 from a
//! missing `/up` still means "the process answers".
//!
//! The Kubernetes manifest names its objects with `@K8S_NAME@`: the project
//! name with `_` replaced by `-`, because a Kubernetes object name and a
//! container name must be DNS labels and `blog_2` is not one.

use brocanth_scaffold::{Category, Manifest, Module, Project, Scaffold};

pub struct Docker;

impl Module for Docker {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "docker",
            "a cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up",
        )
        .category(Category::External)
        .asked_in("operations", "A container image")
        .recommends(
            "observability-logs",
            "the image sets <APP>_ENV=production; without it production writes no log line a machine can read",
        )
        .recommends(
            "observability-health",
            "the HEALTHCHECK calls /up; without it every check is a 404 that passes, so a hung process looks alive",
        )
        .adapts_to(
            "observability-health",
            "the HEALTHCHECK fails on a non-2xx answer from /up (curl -f)",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let curl = if project.has("observability-health") {
            "curl -fsS"
        } else {
            "curl -sS"
        };
        s.file(
            "Dockerfile",
            fill(DOCKERFILE, project).replace("@CURL@", curl),
        );
        s.file(".dockerignore", DOCKERIGNORE.to_string());
    }
}

pub struct Kubernetes;

impl Module for Kubernetes {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "docker-kubernetes",
            "deploy/kubernetes.yaml: a Deployment and a Service whose probes and grace period match the drain",
        )
        .category(Category::External)
        .asked_in("operations", "A Kubernetes deployment of the image")
        .part_of_but_off("docker")
        .because("it deploys the image the Dockerfile builds")
        .needs(
            "observability-health",
            "its liveness and readiness probes are `/up` and `/readyz`",
        )
        // Not health: that one is a need above, and a need is stronger.
        .recommends(
            "observability-logs",
            "the pods run with <APP>_ENV=production; without it the cluster collects no JSON log line",
        )
        .adapts_to(
            "observability-metrics",
            "/metrics moves to port 9090, which the Service does not expose",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let metrics = if project.has("observability-metrics") {
            METRICS_ENV
        } else {
            ""
        };
        let yaml = KUBERNETES_YAML.replace("@METRICS@", metrics);
        s.file("deploy/kubernetes.yaml", fill(&yaml, project));
    }
}

fn fill(template: &str, project: &Project) -> String {
    template
        .replace("@RUST_VERSION@", brocanth_scaffold::RUST_VERSION)
        .replace("@K8S_NAME@", &k8s_name(project.name))
        .replace("@NAME@", project.name)
        .replace("@TITLE@", project.title)
        .replace("@UPPER@", &project.name.replace('-', "_").to_uppercase())
}

/// `name` as a DNS label (RFC 1123), which a Kubernetes object name and a
/// container name must be. A project name is lowercase letters, digits, `-`
/// and `_`, so only `_` needs a change.
fn k8s_name(name: &str) -> String {
    name.replace('_', "-").to_ascii_lowercase()
}

/// The container env and port that move `/metrics` off the public listener.
///
/// Without them, `/metrics` answers on the port the Service exposes, with
/// no token. A scrape inside the cluster reaches the pod on 9090 directly.
const METRICS_ENV: &str = "            - name: @UPPER@_OBSERVABILITY__METRICS_BIND
              value: \"0.0.0.0:9090\"
";

const DOCKERFILE: &str = r#"# syntax=docker/dockerfile:1
#
# The image of @TITLE@.
#
#   docker build -t @NAME@ .
#   docker run --rm -p 3000:3000 @NAME@
#
# Three stages. `planner` reduces the manifests to a recipe. `builder` builds
# the dependencies from the recipe, in a layer that stays cached until
# Cargo.toml or Cargo.lock changes, and then builds the application. The
# runtime holds the binary, the config directory, the migrations directory
# (`migrate up` reads SQL migrations from it) and nothing else.
#
# Dependencies on a path outside this directory (`brocanth create
# --brocanth-path`) are not in the build context, and the build fails. Build
# from git dependencies, or add the checkout to the context.

ARG RUST_VERSION=@RUST_VERSION@

FROM rust:${RUST_VERSION}-slim-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin @NAME@ \
 && mkdir -p config migrations

FROM debian:bookworm-slim AS runtime
# ca-certificates for TLS to a database or an API; curl for HEALTHCHECK.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin app
WORKDIR /app
COPY --from=builder /app/target/release/@NAME@ /app/@NAME@
COPY --from=builder /app/config /app/config
COPY --from=builder /app/migrations /app/migrations
USER 10001

# Every interface, inside the container. `$@UPPER@_ENV` picks
# config/production.toml, and a variable such as `$@UPPER@_DB__URL`
# overrides one key of it.
ENV @UPPER@_ADDR=0.0.0.0:3000 \
    @UPPER@_ENV=production
EXPOSE 3000

# `/up` is the liveness probe of `observability-health`. With that module,
# `curl -f` fails the check on any status that is not 2xx. Without it, any
# HTTP answer passes, so the 404 from a missing `/up` means "the process
# answers".
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
  CMD @CURL@ -o /dev/null --max-time 2 http://127.0.0.1:3000/up || exit 1

# `serve` drains on SIGTERM for `[server] shutdown_grace_secs` (25 by
# default). Give `docker stop` more than that: `docker stop -t 30`.
STOPSIGNAL SIGTERM
CMD ["/app/@NAME@", "serve"]
"#;

/// What the build context does not need.
const DOCKERIGNORE: &str = "/target
/.git
/db
/node_modules
";

const KUBERNETES_YAML: &str = r#"# A minimal deployment of this image. Change the image, the replica count
# and the secrets; keep the probes and the timings in step.
#
# The stop, in order:
#   1. SIGTERM. `/readyz` answers 503 at once, so the Service stops routing
#      here. The listener stays open for `drain_delay_secs` (5 below), while
#      the endpoint change reaches every node.
#   2. The listener closes. Open requests have what is left of
#      `shutdown_grace_secs` (25) to finish, then the plugins stop.
#   3. terminationGracePeriodSeconds (30) is the hard limit: SIGKILL. It is
#      above the 25, so a clean drain never meets it.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: @K8S_NAME@
spec:
  replicas: 2
  selector:
    matchLabels:
      app: @K8S_NAME@
  template:
    metadata:
      labels:
        app: @K8S_NAME@
    spec:
      terminationGracePeriodSeconds: 30
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
      containers:
        - name: @K8S_NAME@
          image: @NAME@:latest
          ports:
            - name: http
              containerPort: 3000
          env:
            - name: @UPPER@_SERVER__SHUTDOWN_GRACE_SECS
              value: "25"
            - name: @UPPER@_OBSERVABILITY__DRAIN_DELAY_SECS
              value: "5"
            # The probe paths below, pinned here so a config file that moves
            # them cannot leave the probes asking for a 404.
            - name: @UPPER@_OBSERVABILITY__LIVE_PATH
              value: /up
            - name: @UPPER@_OBSERVABILITY__READY_PATH
              value: /readyz
@METRICS@          livenessProbe:
            httpGet:
              path: /up
              port: http
            periodSeconds: 10
          # `/readyz` runs every plugin check, each with a 2 s budget
          # (`check_timeout`). The probe waits 3 s, and one slow check does
          # not drop every replica at once: it takes three misses. The drain
          # is unaffected, because `/readyz` answers 503 for the whole
          # `drain_delay_secs`.
          readinessProbe:
            httpGet:
              path: /readyz
              port: http
            periodSeconds: 2
            timeoutSeconds: 3
            failureThreshold: 3
---
apiVersion: v1
kind: Service
metadata:
  name: @K8S_NAME@
spec:
  selector:
    app: @K8S_NAME@
  ports:
    - name: http
      port: 80
      targetPort: http
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "deploy/kubernetes.yaml";

    fn written(module: &dyn Module, name: &str, on: &[&str], path: &str) -> String {
        let known = crate::modules::keys();
        let project = Project::new(name, "Demo", on, &known);
        let mut s = Scaffold::new();
        module.contribute(&project, &mut s);
        let text = s
            .files()
            .find(|(p, _)| *p == path)
            .map(|(_, text)| text.to_string());
        text.expect("the module writes the file")
    }

    #[test]
    fn a_name_with_an_underscore_is_a_dns_label_in_the_manifest() {
        let yaml = written(&Kubernetes, "blog_2", &["observability-health"], YAML);
        assert!(yaml.contains("  name: blog-2\n"), "{yaml}");
        assert!(yaml.contains("        - name: blog-2\n"), "{yaml}");
        assert!(yaml.contains("app: blog-2"), "{yaml}");
        // The image keeps the binary's own name, and the env the app's.
        assert!(yaml.contains("image: blog_2:latest"), "{yaml}");
        assert!(
            yaml.contains("BLOG_2_SERVER__SHUTDOWN_GRACE_SECS"),
            "{yaml}"
        );
        assert!(!yaml.contains('@'), "a placeholder was left behind: {yaml}");
    }

    #[test]
    fn one_slow_check_does_not_drop_every_replica() {
        let yaml = written(&Kubernetes, "blog", &["observability-health"], YAML);
        assert!(yaml.contains("timeoutSeconds: 3"), "{yaml}");
        assert!(yaml.contains("failureThreshold: 3"), "{yaml}");
        assert!(yaml.contains("BLOG_OBSERVABILITY__READY_PATH"), "{yaml}");
    }

    #[test]
    fn metrics_leave_the_public_port_only_when_the_project_has_them() {
        let on = ["observability-health", "observability-metrics"];
        let with = written(&Kubernetes, "blog", &on, YAML);
        assert!(with.contains("BLOG_OBSERVABILITY__METRICS_BIND"), "{with}");
        let without = written(&Kubernetes, "blog", &["observability-health"], YAML);
        assert!(!without.contains("METRICS_BIND"), "{without}");
    }

    #[test]
    fn the_healthcheck_fails_on_an_error_status_only_with_health() {
        let with = written(&Docker, "blog", &["observability-health"], "Dockerfile");
        assert!(with.contains("CMD curl -fsS"), "{with}");
        let without = written(&Docker, "blog", &[], "Dockerfile");
        assert!(without.contains("CMD curl -sS"), "{without}");
        let pinned = format!("RUST_VERSION={}", brocanth_scaffold::RUST_VERSION);
        assert!(without.contains(&pinned), "{without}");
        assert!(!without.contains('@'), "a placeholder was left behind");
    }
}
