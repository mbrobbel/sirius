cmake_minimum_required(VERSION 3.30.4)
project(sirius_bundle_test LANGUAGES CXX)
include(CTest)
include("${CMAKE_CURRENT_LIST_DIR}/../../../cmake/sirius-static-bundle.cmake")
option(BUNDLE_WITH_SHARED_DEP "Exercise rejection of a shared dependency" OFF)
option(BUNDLE_EXTRA_ROOT "Include an independent archive root" OFF)
set(kind STATIC)
if(BUNDLE_WITH_SHARED_DEP)
  set(kind SHARED)
endif()
add_library(leaf ${kind} two/value.cpp registration.cpp)
add_library(bundle::leaf ALIAS leaf)
add_library(inactive SHARED EXCLUDE_FROM_ALL two/value.cpp)
add_library(core STATIC one/value.cpp standalone_loader.cpp)
target_link_libraries(
  core
  PUBLIC
    "$<LINK_ONLY:$<LINK_LIBRARY:WHOLE_ARCHIVE,$<IF:$<BOOL:0>,inactive,bundle::leaf>>>"
    m
    "$<COMPILE_ONLY:inactive>")
set(bundle_roots core)
if(BUNDLE_EXTRA_ROOT)
  add_library(extra_root STATIC extra_root.cpp)
  list(APPEND bundle_roots extra_root)
endif()
sirius_add_static_bundle(combined "${CMAKE_CURRENT_BINARY_DIR}/libcombined.a"
                         ${bundle_roots})
add_executable(consumer main.cpp)
add_library(archive STATIC IMPORTED)
set_target_properties(
  archive PROPERTIES IMPORTED_LOCATION
                     "${CMAKE_CURRENT_BINARY_DIR}/libcombined.a")
add_dependencies(archive combined)
target_link_libraries(consumer PRIVATE "$<LINK_LIBRARY:WHOLE_ARCHIVE,archive>")
add_test(NAME combined_archive COMMAND consumer)

add_library(host_loader STATIC host_loader.cpp)
add_library(wrapper INTERFACE)
target_link_libraries(wrapper INTERFACE "$<TARGET_OBJECTS:host_loader>" archive)
add_executable(host_consumer host_main.cpp)
target_link_libraries(host_consumer PRIVATE "$<LINK_LIBRARY:WHOLE_ARCHIVE,leaf>"
                                            wrapper)
target_link_options(host_consumer PRIVATE "LINKER:--allow-multiple-definition")
add_test(NAME host_registration_precedence COMMAND host_consumer)

add_test(
  NAME reject_shared_dependency
  COMMAND
    "${CMAKE_COMMAND}" "-DSOURCE=${CMAKE_CURRENT_SOURCE_DIR}"
    "-DBINARY=${CMAKE_CURRENT_BINARY_DIR}/reject-shared"
    "-DCOMPILER=${CMAKE_CXX_COMPILER}" -P
    "${CMAKE_CURRENT_SOURCE_DIR}/reject_shared.cmake")
add_test(
  NAME remove_bundle_root
  COMMAND
    "${CMAKE_COMMAND}" "-DSOURCE=${CMAKE_CURRENT_SOURCE_DIR}"
    "-DBINARY=${CMAKE_CURRENT_BINARY_DIR}/remove-root"
    "-DCOMPILER=${CMAKE_CXX_COMPILER}" "-DARCHIVER=${CMAKE_AR}" -P
    "${CMAKE_CURRENT_SOURCE_DIR}/remove_root.cmake")
