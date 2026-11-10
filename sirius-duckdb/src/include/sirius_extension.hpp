#pragma once

#include "duckdb.hpp"

namespace duckdb {

class SiriusExtension : public Extension {
 public:
  void Load(ExtensionLoader& db) override;
  std::string Name() override;
  std::string Version() const override;
  /// What duckdb_extensions() shows while this build is loaded.
  static std::string Description();
};

}  // namespace duckdb
