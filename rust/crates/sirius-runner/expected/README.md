# Expected results

Validation data, keyed by the *logical* dataset instance:

```
expected/<suite>/sf<N>/<query>.*
```

Expected results depend on the benchmark and scale factor but not on the
storage format, compression, or encoding — one expected-set serves every
dataset variant at that scale factor.

Unlike `datasets/`, `suites/`, and `benches/`, this directory is **not**
embedded in the binary. Resolution order at validation time: this directory
(or `<--assets>/expected/`) → the local cache under the data root → generate
via the suite's reference engine and cache. Small/common scale factors are
committed here so CI and developers never run the reference engine; large
scale factors may later move to a dedicated validation-results repository
using the same layout.

Populated by `sirius-runner validate generate <suite> --scale-factor <N>`.
