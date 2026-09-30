cmake_minimum_required(VERSION 3.31 FATAL_ERROR)
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

set(EXTENSION_SOURCES src/sirius/sirius_extension.cpp)
build_static_extension(sirius ${EXTENSION_SOURCES})
build_loadable_extension(sirius " " ${EXTENSION_SOURCES})
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
  get_target_property(links sirius_loadable_extension LINK_LIBRARIES)
  list(REMOVE_ITEM links "-Wl,--exclude-libs,ALL" duckdb_static)
  set_target_properties(
    sirius_loadable_extension
    PROPERTIES LINK_LIBRARIES "${links}"
               LINK_LIBRARY_OVERRIDE_sirius::sirius_static_archive WHOLE_ARCHIVE
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

if(SIRIUS_DUCKDB_LINKAGE STREQUAL "static")
  add_custom_command(
    TARGET sirius_loadable_extension
    POST_BUILD
    COMMAND
      "${CMAKE_COMMAND}" -DREMOVE_RPATH=ON
      "-DEXTENSION=$<TARGET_FILE:sirius_loadable_extension>" -P
      "${CMAKE_CURRENT_SOURCE_DIR}/cmake/check-static-extension.cmake"
    VERBATIM)
endif()
