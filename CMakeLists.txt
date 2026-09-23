cmake_minimum_required(VERSION 4.4 FATAL_ERROR)

project(sirius-duckdb)

set(EXTENSION_SOURCES src/sirius/sirius_extension.cpp)

build_static_extension(sirius ${EXTENSION_SOURCES})
build_loadable_extension(sirius " " ${EXTENSION_SOURCES})

foreach(target IN ITEMS sirius_extension sirius_loadable_extension)
  set_target_properties(${target} PROPERTIES CXX_STANDARD 20 CXX_STANDARD_REQUIRED ON)
  target_include_directories(${target} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/src/include)
endforeach()

install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
