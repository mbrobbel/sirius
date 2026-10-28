// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once
#include "sirius_context.hpp"
struct sirius_context {
  duckdb::shared_ptr<duckdb::SiriusContext> engine =
    duckdb::make_shared_ptr<duckdb::SiriusContext>();
};
