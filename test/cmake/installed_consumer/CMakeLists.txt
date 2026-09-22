cmake_minimum_required(VERSION 3.30.4)
project(sirius_installed_consumer LANGUAGES CXX)
set(SIRIUS_TEST_LINKAGE
    "shared"
    CACHE STRING "Installed library to test")
find_package(sirius REQUIRED CONFIG COMPONENTS ${SIRIUS_TEST_LINKAGE})
add_executable(consumer main.cpp)
if(SIRIUS_TEST_LINKAGE STREQUAL "static")
  target_link_libraries(consumer PRIVATE sirius::sirius_static)
else()
  target_link_libraries(consumer PRIVATE sirius::sirius)
endif()
