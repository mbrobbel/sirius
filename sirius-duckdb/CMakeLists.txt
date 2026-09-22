cmake_minimum_required(VERSION 3.30.4)
project(sirius_duckdb LANGUAGES CXX)

if(NOT COMMAND build_loadable_extension)
  message(
    FATAL_ERROR
      "Configure the DuckDB source tree with sirius-duckdb/extension_config.cmake"
  )
endif()
find_package(sirius REQUIRED CONFIG)
sirius_check_duckdb_compatibility("${DuckDB_SOURCE_DIR}")

build_static_extension(sirius src/sirius_extension.cpp)
build_loadable_extension(sirius CPP src/sirius_extension.cpp)
foreach(target sirius_extension sirius_loadable_extension)
  target_include_directories(${target}
                             PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/src/include")
  target_link_libraries(${target} sirius::sirius)
  set_target_properties(${target} PROPERTIES CXX_STANDARD 20
                                             CXX_STANDARD_REQUIRED ON)
endforeach()
install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
