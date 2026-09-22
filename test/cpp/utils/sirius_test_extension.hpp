// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once

#include "sirius/duckdb.hpp"

#include <duckdb.hpp>

namespace duckdb {

class SiriusTestExtension : public Extension {
 public:
  void Load(ExtensionLoader& loader) override { sirius::register_duckdb_extension(loader); }
  std::string Name() override { return "Sirius\tExtension"; }
  std::string Version() const override { return "test"; }
};

}  // namespace duckdb
