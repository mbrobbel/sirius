# Carry the selected toolchain into the separate, host-only extension build.
set(_sirius_duckdb_cache "")
foreach(
  _setting
  CMAKE_BUILD_TYPE
  CMAKE_EXPORT_COMPILE_COMMANDS
  CMAKE_TOOLCHAIN_FILE
  CMAKE_PREFIX_PATH
  CMAKE_C_COMPILER
  CMAKE_CXX_COMPILER
  CMAKE_C_COMPILER_LAUNCHER
  CMAKE_CXX_COMPILER_LAUNCHER
  CMAKE_LINKER_TYPE
  CMAKE_C_FLAGS
  CMAKE_CXX_FLAGS
  CMAKE_EXE_LINKER_FLAGS
  CMAKE_SHARED_LINKER_FLAGS
  CMAKE_MODULE_LINKER_FLAGS
  ENABLE_SANITIZER
  ENABLE_UBSAN
  ENABLE_THREAD_SANITIZER
  EXPORT_DYNAMIC_SYMBOLS
  CMAKE_POLICY_VERSION_MINIMUM
  VCPKG_BUILD
  VCPKG_MANIFEST_DIR
  VCPKG_INSTALLED_DIR
  VCPKG_TARGET_TRIPLET
  VCPKG_HOST_TRIPLET)
  if(DEFINED ${_setting})
    string(APPEND _sirius_duckdb_cache
           "set(${_setting} [==[${${_setting}}]==] CACHE STRING \"\" FORCE)\n")
  endif()
endforeach()
foreach(_language C CXX)
  foreach(_mode DEBUG RELEASE RELWITHDEBINFO MINSIZEREL)
    set(_setting "CMAKE_${_language}_FLAGS_${_mode}")
    string(APPEND _sirius_duckdb_cache
           "set(${_setting} [==[${${_setting}}]==] CACHE STRING \"\" FORCE)\n")
  endforeach()
endforeach()
set(_sirius_linkage shared)
if(SIRIUS_BUILD_STATIC)
  set(_sirius_linkage static)
endif()
string(
  APPEND
  _sirius_duckdb_cache
  "set(sirius_DIR [==[${CMAKE_BINARY_DIR}/install/${CMAKE_INSTALL_LIBDIR}/cmake/sirius]==] CACHE PATH \"\" FORCE)\n"
  "set(DUCKDB_EXTENSION_CONFIGS [==[${PROJECT_SOURCE_DIR}/sirius-duckdb/extension_config.cmake]==] CACHE STRING \"\" FORCE)\n"
  "set(SIRIUS_DUCKDB_LINKAGE ${_sirius_linkage} CACHE STRING \"\" FORCE)\n"
  "set(STATICALLY_LINK_EXTENSIONS [==[core_functions;parquet]==] CACHE STRING \"\" FORCE)\n"
  "set(EXTENSION_STATIC_BUILD ON CACHE BOOL \"\" FORCE)\n"
  "set(CMAKE_CXX_SCAN_FOR_MODULES OFF CACHE BOOL \"\" FORCE)\n"
  "set(VCPKG_MANIFEST_INSTALL OFF CACHE BOOL \"\" FORCE)\n")
file(
  GENERATE
  OUTPUT "${CMAKE_BINARY_DIR}/sirius-duckdb-cache.cmake"
  CONTENT "${_sirius_duckdb_cache}")
