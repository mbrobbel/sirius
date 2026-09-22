cmake_minimum_required(VERSION 3.30.4)
project(sirius_duckdb LANGUAGES CXX)

if(NOT COMMAND build_loadable_extension)
  message(
    FATAL_ERROR
      "Configure the DuckDB source tree with sirius-duckdb/extension_config.cmake"
  )
endif()
set(SIRIUS_DUCKDB_LINKAGE
    "shared"
    CACHE STRING "Sirius dependency: shared or static")
set_property(CACHE SIRIUS_DUCKDB_LINKAGE PROPERTY STRINGS shared static)
if(NOT SIRIUS_DUCKDB_LINKAGE MATCHES "^(shared|static)$")
  message(FATAL_ERROR "SIRIUS_DUCKDB_LINKAGE must be shared or static")
endif()
find_package(sirius REQUIRED CONFIG COMPONENTS ${SIRIUS_DUCKDB_LINKAGE})
set(sirius_dependency sirius::sirius)
if(SIRIUS_DUCKDB_LINKAGE STREQUAL "static")
  set(sirius_dependency sirius::sirius_static)
endif()
sirius_check_duckdb_compatibility("${DuckDB_SOURCE_DIR}")

build_static_extension(sirius src/sirius_extension.cpp)
build_loadable_extension(sirius CPP src/sirius_extension.cpp)
foreach(target sirius_extension sirius_loadable_extension)
  target_include_directories(${target}
                             PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/src/include")
  target_link_libraries(${target} ${sirius_dependency})
  set_target_properties(${target} PROPERTIES CXX_STANDARD 20
                                             CXX_STANDARD_REQUIRED ON)
endforeach()
install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")

if(SIRIUS_DUCKDB_LINKAGE STREQUAL "static")
  # Host registration must precede the standalone loader in the combined
  # archive.
  target_link_libraries(
    sirius_extension
    "$<BUILD_INTERFACE:$<TARGET_OBJECTS:duckdb_generated_extension_loader>>")
  get_target_property(links sirius_loadable_extension LINK_LIBRARIES)
  list(REMOVE_ITEM links "-Wl,--exclude-libs,ALL" duckdb_static
       dummy_static_extension_loader)
  set_target_properties(
    sirius_loadable_extension
    PROPERTIES LINK_LIBRARIES "${links}"
               LINKER_TYPE BFD
               SKIP_BUILD_RPATH ON)
  # Keep NVTX and the RMM resource registry visible while hiding bundled
  # internals.
  target_link_options(
    sirius_loadable_extension PRIVATE
    "LINKER:--version-script=${CMAKE_CURRENT_SOURCE_DIR}/exports.map")
  set_property(
    TARGET sirius_loadable_extension
    APPEND
    PROPERTY LINK_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/exports.map")
endif()
