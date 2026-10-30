//! `docker`: a container image for the project, and `docker-kubernetes`, a
//! minimal deployment of it.
//!
//! A module rather than a file every project gets, because the image is one
//! way to ship among several, and a project that deploys some other way
//! should not carry a Dockerfile nobody builds.
//!
//! The Dockerfile's text depends on the project name alone, so `brocanth
//! add` never has to change a file the developer tunes: a serve-only binary
//! ignores the `serve` argument, the config directory is made empty when
//! there is none, and the health check accepts any HTTP answer, so a
//! project without `observability-health` passes it with a 404 from `/up`.

use brocanth_scaffold::{Manifest, Module, Project, Scaffold};

pub struct Docker;

impl Module for Docker {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "docker",
            "a cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up",
        )
        .asked_in("operations", "A container image")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("Dockerfile", fill(DOCKERFILE, project));
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
        .asked_in("operations", "A Kubernetes deployment of the image")
        .part_of_but_off("docker")
        .because("it deploys the image the Dockerfile builds")
        .needs(
            "observability-health",
            "its liveness and readiness probes are `/up` and `/readyz`",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("deploy/kubernetes.yaml", fill(KUBERNETES_YAML, project));
    }
}

fn fill(template: &str, project: &Project) -> String {
    template
        .replace("@NAME@", project.name)
        .replace("@TITLE@", project.title)
        .replace("@UPPER@", &project.name.replace('-', "_").to_uppercase())
}

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
# runtime holds the binary, the config directory and nothing else.
#
# Dependencies on a path outside this directory (`brocanth create
# --brocanth-path`) are not in the build context, and the build fails. Build
# from git dependencies, or add the checkout to the context.

ARG RUST_VERSION=1.98

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
 && mkdir -p config

FROM debian:bookworm-slim AS runtime
# ca-certificates for TLS to a database or an API; curl for HEALTHCHECK.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin app
WORKDIR /app
COPY --from=builder /app/target/release/@NAME@ /app/@NAME@
COPY --from=builder /app/config /app/config
USER 10001

# Every interface, inside the container. `$@UPPER@_ENV` picks
# config/production.toml, and a variable such as `$@UPPER@_MODEL__URL`
# overrides one key of it.
ENV @UPPER@_ADDR=0.0.0.0:3000 \
    @UPPER@_ENV=production
EXPOSE 3000

# `/up` is the liveness probe of `observability-health`. Any HTTP answer
# passes, so a project without it passes with a 404: the process answers.
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -sS -o /dev/null --max-time 2 http://127.0.0.1:3000/up || exit 1

# `serve` drains on SIGTERM for `[server] shutdown_grace_secs` (25 by
# default). Give `docker stop` more than that: `docker stop -t 30`.
STOPSIGNAL SIGTERM
CMD ["/app/@NAME@", "serve"]
"#;

/// What the build context does not need.
const DOCKERIGNORE: &str = "/target
/.git
/db
*.sqlite
*.sqlite-*
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
  name: @NAME@
spec:
  replicas: 2
  selector:
    matchLabels:
      app: @NAME@
  template:
    metadata:
      labels:
        app: @NAME@
    spec:
      terminationGracePeriodSeconds: 30
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
      containers:
        - name: @NAME@
          image: @NAME@:latest
          ports:
            - name: http
              containerPort: 3000
          env:
            - name: @UPPER@_SERVER__SHUTDOWN_GRACE_SECS
              value: "25"
            - name: @UPPER@_OBSERVE__DRAIN_DELAY_SECS
              value: "5"
          livenessProbe:
            httpGet:
              path: /up
              port: http
            periodSeconds: 10
          readinessProbe:
            httpGet:
              path: /readyz
              port: http
            periodSeconds: 2
            failureThreshold: 1
---
apiVersion: v1
kind: Service
metadata:
  name: @NAME@
spec:
  selector:
    app: @NAME@
  ports:
    - name: http
      port: 80
      targetPort: http
"#;
