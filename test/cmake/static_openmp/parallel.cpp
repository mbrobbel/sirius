// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
extern "C" int parallel_sum()
{
  int result = 0;
#pragma omp parallel for reduction(+ : result)
  for (int i = 0; i < 100; ++i) {
    result += i;
  }
  return result;
}
