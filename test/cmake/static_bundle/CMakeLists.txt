cmake_minimum_required(VERSION 3.30.4)
project(sirius_bundle_test LANGUAGES CXX)
include(CTest)
include("${CMAKE_CURRENT_LIST_DIR}/../../../cmake/sirius-static-bundle.cmake")
option(BUNDLE_WITH_SHARED_DEP "Exercise rejection of a shared dependency" OFF)
set(kind STATIC)
if(BUNDLE_WITH_SHARED_DEP)
  set(kind SHARED)
endif()
add_library(leaf ${kind} two/value.cpp registration.cpp)
add_library(bundle::leaf ALIAS leaf)
add_library(core STATIC one/value.cpp)
target_link_libraries(
  core PUBLIC "$<LINK_ONLY:$<LINK_LIBRARY:WHOLE_ARCHIVE,bundle::leaf>>" m)
sirius_add_static_bundle(combined "${CMAKE_CURRENT_BINARY_DIR}/libcombined.a"
                         core)
add_executable(consumer main.cpp)
add_library(archive STATIC IMPORTED)
set_target_properties(
  archive PROPERTIES IMPORTED_LOCATION
                     "${CMAKE_CURRENT_BINARY_DIR}/libcombined.a")
add_dependencies(archive combined)
target_link_libraries(consumer PRIVATE "$<LINK_LIBRARY:WHOLE_ARCHIVE,archive>")
add_test(NAME combined_archive COMMAND consumer)
