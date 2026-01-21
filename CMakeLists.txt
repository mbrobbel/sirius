# =============================================================================
# Copyright 2025, Sirius Contributors.
#
# Licensed under the Apache License, Version 2.0 (the "License"); you may not
# use this file except in compliance with the License. You may obtain a copy of
# the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
# WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
# License for the specific language governing permissions and limitations under
# the License.
# =============================================================================

cmake_minimum_required(VERSION 3.31 FATAL_ERROR)

project(sirius LANGUAGES CXX CUDA)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT DEFINED CMAKE_CUDA_ARCHITECTURES)
  set(CMAKE_CUDA_ARCHITECTURES native)
endif()
set(CMAKE_CUDA_RESOLVE_DEVICE_SYMBOLS ON)
set(CMAKE_CUDA_SEPARABLE_COMPILATION ON)
set(CMAKE_CUDA_STANDARD 20)
set(CMAKE_CUDA_STANDARD_REQUIRED ON)

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

find_package(nanoarrow REQUIRED CONFIG)
find_package(rmm REQUIRED CONFIG)
find_package(cudf REQUIRED CONFIG)
find_package(libconfig REQUIRED CONFIG)
find_package(spdlog REQUIRED CONFIG)

find_package(PkgConfig REQUIRED)
pkg_check_modules(numa REQUIRED IMPORTED_TARGET numa)
add_library(numa ALIAS PkgConfig::numa)

# TODO(mbrobbel): remove this
add_compile_definitions(SIRIUS_DEFAULT_LOG_DIR="${CMAKE_BINARY_DIR}/log")

# Third-party dependencies (must be before include_directories for cuCascade
# headers)
include(third_party/cucascade.cmake)

# Include directories - cuCascade headers first for memory module
# include_directories(${CUCASCADE_INCLUDE_DIR})
include_directories(src/include src/include/operator)

set(EXTENSION_SOURCES
    src/sirius_extension.cpp
    src/gpu_context.cpp
    src/gpu_executor.cpp
    src/gpu_physical_operator.cpp
    src/gpu_pipeline.cpp
    src/gpu_meta_pipeline.cpp
    src/gpu_physical_plan_generator.cpp
    src/gpu_buffer_manager.cpp
    src/gpu_columns.cpp
    src/cpu_cache.cpp
    src/gpu_query_result.cpp
    src/config.cpp
    src/fallback.cpp
    src/sirius_config.cpp
    src/sirius_context.cpp
    src/extension_lock.cpp
    src/parallel/task_executor.cpp)
# Note: Memory module sources are now provided by cuCascade library
add_subdirectory(src/operator)
add_subdirectory(src/plan)
add_subdirectory(src/data)
add_subdirectory(src/pipeline)
add_subdirectory(src/downgrade)
add_subdirectory(src/creator)
add_subdirectory(src/op)
add_subdirectory(src/planner)
add_subdirectory(src/memory)

set(CUDA_SOURCES src/cuda/communication.cu src/cuda/allocator.cu
                 src/cuda/print.cu src/cuda/utils.cu)
add_subdirectory(src/cuda/operator)
add_subdirectory(src/cuda/cudf)
add_subdirectory(src/cuda/expression_executor)
add_subdirectory(src/expression_executor)

build_static_extension(sirius ${EXTENSION_SOURCES} ${CUDA_SOURCES})
build_loadable_extension(sirius " " ${EXTENSION_SOURCES}
                         ${CUDA_SOURCES})

add_dependencies(sirius_extension duckdb_static)
add_dependencies(sirius_loadable_extension duckdb_static)

set_target_properties(sirius_extension PROPERTIES LINKER_LANGUAGE CUDA)
set_target_properties(sirius_loadable_extension PROPERTIES LINKER_LANGUAGE
                                                            CUDA)

target_link_libraries(sirius_extension
    cudf::cudf
    rmm::rmm
    numa
    libconfig::config++
    spdlog::spdlog
    cuCascade::cucascade
)
target_link_libraries(sirius_loadable_extension
    cudf::cudf
    rmm::rmm
    numa
    spdlog::spdlog
    cuCascade::cucascade
)

# Install cucascade_static to the DuckDB export set (required for
# sirius_extension export)
install(
  TARGETS cucascade_static
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")

install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")

# Unit tests
set(TEST_NAME sirius_unittest)
add_subdirectory(test/cpp)
add_executable(${TEST_NAME} ${TEST_SOURCES})
target_include_directories(${TEST_NAME} PRIVATE duckdb/third_party/catch
                                                test/cpp)
target_link_libraries(${TEST_NAME} sirius_extension)
set(TEST_EXEC_DIR "${CMAKE_CURRENT_BINARY_DIR}/test/cpp")
set(TEST_LOG_DIR "${CMAKE_CURRENT_BINARY_DIR}/test/cpp/log")
set_target_properties(${TEST_NAME} PROPERTIES RUNTIME_OUTPUT_DIRECTORY
                                              "${TEST_EXEC_DIR}")
target_compile_definitions(${TEST_NAME}
                           PRIVATE SIRIUS_UNITTEST_LOG_DIR="${TEST_LOG_DIR}")
