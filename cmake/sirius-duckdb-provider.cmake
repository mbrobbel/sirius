# DuckDB remains a source dependency until its package exposes the required
# internal headers and extension libraries. Keep that contract in one place.
set(SIRIUS_DUCKDB_SOURCE_DIR
    "${CMAKE_CURRENT_SOURCE_DIR}/duckdb"
    CACHE PATH "DuckDB source tree used to build Sirius")

if(NOT EXISTS "${SIRIUS_DUCKDB_SOURCE_DIR}/src/include/duckdb.hpp")
  message(
    FATAL_ERROR
      "Initialize the DuckDB submodule or set SIRIUS_DUCKDB_SOURCE_DIR")
endif()

include("${CMAKE_CURRENT_LIST_DIR}/sirius-source-revision.cmake")
sirius_source_revision("${SIRIUS_DUCKDB_SOURCE_DIR}" SIRIUS_DUCKDB_REVISION)

function(sirius_add_duckdb_source)
  # Keep DuckDB options scoped to its dependency build.
  set(DUCKDB_EXTENSION_CONFIGS "")
  set(BUILD_EXTENSIONS "core_functions;parquet")
  set(BUILD_SHELL OFF)
  set(BUILD_UNITTESTS OFF)
  string(SUBSTRING "${SIRIUS_DUCKDB_REVISION}" 0 10 _duckdb_hash)
  set(OVERRIDE_GIT_DESCRIBE "v1.5.5-0-g${_duckdb_hash}")
  add_subdirectory("${SIRIUS_DUCKDB_SOURCE_DIR}" "${CMAKE_BINARY_DIR}/duckdb"
                   EXCLUDE_FROM_ALL)
endfunction()
sirius_add_duckdb_source()

add_library(sirius_duckdb_dependency INTERFACE IMPORTED)
add_library(sirius::duckdb_dependency ALIAS sirius_duckdb_dependency)
get_directory_property(_duckdb_headers DIRECTORY "${SIRIUS_DUCKDB_SOURCE_DIR}"
                                                 INCLUDE_DIRECTORIES)
get_directory_property(
  _duckdb_definitions DIRECTORY "${SIRIUS_DUCKDB_SOURCE_DIR}"
                                COMPILE_DEFINITIONS)
set_target_properties(
  sirius_duckdb_dependency
  PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES
    "${_duckdb_headers};${SIRIUS_DUCKDB_SOURCE_DIR}/extension/core_functions/include;${SIRIUS_DUCKDB_SOURCE_DIR}/extension/parquet/include"
    INTERFACE_COMPILE_DEFINITIONS "${_duckdb_definitions}"
    INTERFACE_LINK_LIBRARIES
    "duckdb_static;core_functions_extension;parquet_extension")

include(CheckCXXSourceCompiles)
foreach(abi 0 1)
  unset(_sirius_abi_matches CACHE)
  check_cxx_source_compiles(
    "#include <string>\nstatic_assert(_GLIBCXX_USE_CXX11_ABI == ${abi});\nint main() {}"
    _sirius_abi_matches)
  if(_sirius_abi_matches)
    set(SIRIUS_LIBSTDCXX_ABI "${abi}")
    break()
  endif()
endforeach()
if(NOT DEFINED SIRIUS_LIBSTDCXX_ABI)
  message(FATAL_ERROR "Sirius requires a supported libstdc++ ABI")
endif()
