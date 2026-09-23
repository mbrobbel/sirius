cmake_minimum_required(VERSION 4.4 FATAL_ERROR)

set(EXTENSION_NAME ${TARGET_NAME}_extension)
set(LOADABLE_EXTENSION_NAME ${TARGET_NAME}_loadable_extension)

project(sirius-duckdb)

set(CMAKE_CXX_STANDARD "17" CACHE STRING "C++ standard to enforce")
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include_directories(src/include)

set(EXTENSION_SOURCES src/sirius_extension.cpp)

build_static_extension(sirius ${EXTENSION_SOURCES})
build_loadable_extension(sirius " " ${EXTENSION_SOURCES})

install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
