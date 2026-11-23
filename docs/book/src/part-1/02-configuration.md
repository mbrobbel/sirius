# Configuration

Sirius can load engine settings from defaults or a YAML file without touching a GPU. The
C++, C and Rust configuration builders support both. Code overrides for devices, memory and
transport are proposed additions. [Public configuration builder](https://github.com/sirius-db/sirius/blob/ef7eb2951a370f4293784549bc05fc46c7cc64f6/include/sirius/context/config_builder.hpp)

## What configuration is for

A configuration describes how *this process* runs the engine. In YAML, the settings sit in groups
under one top-level `sirius` key:

| Group | What it controls |
|---|---|
| `topology` | Which local GPUs to use: a count (`num_gpus`) or explicit device ids (`gpu_ids`). |
| `memory` | Limits and spill thresholds for the GPU, pinned host and disk memory tiers. |
| `executor` | Thread pools: pipeline threads, scan I/O, spilling and task creation. |
| `operator_params` | Engine tuning, such as batch sizes and size limits for hash tables. |
| `telemetry` | Whether and where the engine records execution telemetry. |

A few settings sit outside these groups, such as compression and the limit on concurrent
executions. See the [engine configuration reference](https://github.com/sirius-db/sirius/blob/ef7eb2951a370f4293784549bc05fc46c7cc64f6/docs/super-sirius/configuration.md) for defaults, units and constraints.

Every setting in a `ContextConfig` applies to the whole engine, and there is one engine per
process. Which settings become per-query options, such as the `PrepareOptions` you pass to
`prepare_with`, is part of
[open question 15](../reference/open-questions.md#15-one-context-per-process). Admission and the
limit on concurrent executions stay engine-level, because
[admission across processes](../part-5/08-running-at-scale.md#admission-across-processes) needs
them.

## Start from defaults

Build a snapshot of the defaults directly.

```cpp
#include <sirius/context/config_builder.hpp>
using namespace sirius;

std::expected<ContextConfig, Error> default_config() {
  return ContextConfigBuilder{}.build();
}
```

```rust
use sirius::{ConfigError, ContextConfig, ContextConfigBuilder};

fn default_config() -> Result<ContextConfig, ConfigError> {
    ContextConfigBuilder::new()?.build()
}
```

By default the engine selects every visible GPU, honoring `CUDA_VISIBLE_DEVICES`. The default
capacity budgets are 95% of each device’s memory and 90% of each backing NUMA node’s RAM for
pinned host memory. Spilling to disk stays off
until you name a directory for it.

`build()` never looks at the hardware. It keeps fractions as fractions and leaves GPU selection
unresolved; `Context::create` resolves both.

## Load a YAML file

Most deployments keep their settings in a file:

```yaml
sirius:
  topology:
    num_gpus: 1
  memory:
    gpu:
      usage_limit_bytes: 8Gi
  executor:
    pipeline:
      num_threads: 4
```

Settings you leave out keep their defaults. Byte values accept suffixes such as `8Gi` (binary) or
`8G` (decimal). Loading the file takes one more call:

```cpp
SIRIUS_TRY(builder, ContextConfigBuilder::from_yaml("sirius.yaml"));
SIRIUS_TRY(config, builder.build());
```

```rust
let builder = ContextConfigBuilder::from_yaml("sirius.yaml")?;
let config = builder.build()?;
```

`from_yaml` parses the file, applies it on top of the built-in defaults, and validates the
result:

- **Unknown keys are rejected.** A typo such as `usage_limt_bytes` is an error.
- **Conflicting settings are rejected.** `num_gpus` and `gpu_ids` are mutually exclusive, and
  so are a byte limit and a fraction limit for the same tier. A spill threshold must sit above
  the level at which spilling stops.
- **The file is read once.** The builder keeps the parsed settings; later changes to the file
  have no effect.
- **Only the path you pass is read.** There is no search through default locations.

A `ContextConfigBuilder` therefore always holds valid settings, and `build()` has nothing left
to reject. It snapshots the settings, leaves the builder unchanged, and can be called any number
of times.

## Handle configuration errors

These three codes distinguish file and configuration errors.

| Code | Rust variant | Meaning |
|---|---|---|
| `configuration_io` | `ConfigError::Io` | The file could not be opened or read. |
| `malformed_yaml` | `ConfigError::MalformedYaml` | The text is not valid YAML. |
| `invalid_configuration` | `ConfigError::InvalidConfiguration` | A setting is unknown, out of range, or conflicts with another. |

Match on the code and show the message to a person. The message names the file and setting when
it can, but its wording may change between releases, so never parse it. This loader explains a
broken file and passes every other error on:

```cpp
Result<ContextConfig> load_config(const std::filesystem::path& path) {
  auto builder = ContextConfigBuilder::from_yaml(path);
  if (!builder) {
    const Error& error = builder.error();
    if (error.code == ErrorCode::malformed_yaml) {
      std::cerr << path << " is not valid YAML: " << error.message << '\n';
    } else if (error.code == ErrorCode::invalid_configuration) {
      std::cerr << path << " has an invalid setting: " << error.message << '\n';
    }
    return std::unexpected(error);
  }
  return builder->build();
}
```

```rust
fn load_config(path: &std::path::Path) -> Result<ContextConfig, ConfigError> {
    let builder = ContextConfigBuilder::from_yaml(path).inspect_err(|error| match error {
        ConfigError::MalformedYaml(message) => {
            eprintln!("{} is not valid YAML: {message}", path.display())
        }
        ConfigError::InvalidConfiguration(message) => {
            eprintln!("{} has an invalid setting: {message}", path.display())
        }
        _ => {}
    })?;
    builder.build()
}
```

`ConfigError` is `#[non_exhaustive]`, like every public enum in the Rust API, so a `match` needs
a catch-all arm (see [Versioning](../reference/extension-points.md#versioning)).
[API at a glance](../reference/api-at-a-glance.md#errors) lists its other variants, and
[Results and errors](../conventions.md#results-and-errors) covers the error types of both
languages.

## Layer settings in code

The proposal adds three setters for values supplied by a launcher or application.

- `with_devices(DeviceSelection)` chooses the GPUs: `DeviceSelection::all()` or
  `DeviceSelection::ids({...})`. Like the `gpu_ids` key, `ids` rejects an empty list,
  duplicates and negative ids with `invalid_configuration`.
- `with_memory(MemoryLimits)` sets limits for the memory tiers. The exact shape of
  `MemoryLimits` is still open; it will mirror the `memory` section of the YAML schema.
- `with_transport(TransportConfig)` chooses how this process reaches other processes:
  `TransportConfig::in_process()` for a single process, or `TransportConfig::network(...)` with
  network options such as the address to listen on.

The layers apply in a fixed order: **built-in defaults, then YAML, then setters**. A later layer
replaces what an earlier layer said about the same thing. If the file selects one GPU and the
code selects two, you get two.

```cpp
SIRIUS_TRY(builder, ContextConfigBuilder::from_yaml("sirius.yaml"));
SIRIUS_TRY(devices, DeviceSelection::ids({0, 1}));
SIRIUS_TRY(config, std::move(builder)
                       .with_devices(std::move(devices))
                       .with_transport(TransportConfig::in_process())
                       .build());
```

```rust
let builder = ContextConfigBuilder::from_yaml("sirius.yaml")?;
let devices = DeviceSelection::ids(vec![0, 1])?;
let config = builder
    .with_devices(devices)
    .with_transport(TransportConfig::in_process())
    .build()?;
```

The setters follow the [shared API conventions](../conventions.md#shared-api-conventions): they
consume the builder (hence the `std::move(builder)` in C++) and cannot fail. Anything that can
fail is in the argument, built by a fallible factory such as `DeviceSelection::ids`.

To derive several configurations from one base, copy the base first. Copying a builder
(`clone()` in Rust) is cheap and cannot fail, because copies share their immutable settings.

## A snapshot with no getters

A `ContextConfig` keeps its settings after the builder is dropped or the file changes. A
fractional limit stays a fraction until context creation, when the engine can measure the GPU.
A launcher can check the YAML on any machine. A valid file can still request GPU 3 on a host
that has only two GPUs, in which case context creation fails.

## Process settings and plan topology

The YAML `topology` section selects the GPUs available to this process. A distributed plan uses two more pieces of information.

- **Worker sets live in the plan.** A plan says that it runs on `compute[4]`, a set of four
  logical workers; see
  [Workers and worker sets](../part-2/02-workers-and-distribution.md#workers-and-worker-sets).
- **Endpoints live in the bindings.** Your program or launcher binds each logical worker to an
  endpoint, a specific GPU in a specific process, at prepare time; see
  [Bindings](../part-2/04-stages-and-bindings.md#bindings-where-never-what).

Memory limits, thread counts and batch sizes tune how the engine executes a plan. Processes with
different memory budgets can participate in the same exchange because the plan supplies the
partitioning and schema.

Today a few operator parameters still steer choices that the plan will own, such as the build
size below which a join broadcasts its build side instead of hash-partitioning it, or whether the
engine creates runtime filters on join edges.

The proposed `with_transport` configures the local transport, including its listening address.
The peer for each exchange comes from the plan and its bindings.
