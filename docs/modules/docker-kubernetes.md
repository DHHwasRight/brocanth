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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/docker_kubernetes.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`observability-health`](observability-health.md): its liveness and readiness probes are `/up` and `/readyz` |
| After | [`observability-health`](observability-health.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`observability-metrics`](observability-metrics.md): /metrics moves to port 9090, which the Service does not expose |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `deploy/kubernetes.yaml`

## Settings

None.

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-cli/src/modules/docker.rs`](../../crates/brocanth-cli/src/modules/docker.rs)
