# `docker-kubernetes`

Deploy/kubernetes.yaml: a Deployment and a Service whose probes and grace period match the drain.

The wizard calls it: *A Kubernetes deployment of the image*.

| | |
|---|---|
| Categories | **External** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | [`docker`](docker.md), off until you tick it |
| Needs | [`observability-health`](observability-health.md): its liveness and readiness probes are `/up` and `/readyz` |
| After | [`observability-health`](observability-health.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

## What it writes

Files:

- `deploy/kubernetes.yaml`

## Settings

None.

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-cli/src/modules/docker.rs`](../../crates/brocanth-cli/src/modules/docker.rs)
