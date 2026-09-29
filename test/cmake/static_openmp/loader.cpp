// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include <dlfcn.h>

#include <cstdio>

int main(int argc, char** argv)
{
  if (argc != 2) { return 1; }
  auto* library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (!library) {
    std::fprintf(stderr, "%s\n", dlerror());
    return 1;
  }
  auto sum = reinterpret_cast<int (*)()>(dlsym(library, "parallel_sum"));
  // Keep the runtime loaded while its worker threads live, as DuckDB does.
  return sum && sum() == 4950 ? 0 : 1;
}
