/*
 * Copyright 2026, Sirius Contributors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 */

#define DUCKDB_EXTENSION_MAIN

#include "duckdb/main/extension/extension_loader.hpp"
#include "sirius/duckdb_extension_loader.hpp"

extern "C" {

DUCKDB_CPP_EXTENSION_ENTRY(sirius, loader) { duckdb::load_sirius_extension(loader); }
}

#ifndef DUCKDB_EXTENSION_MAIN
#error DUCKDB_EXTENSION_MAIN not defined
#endif
