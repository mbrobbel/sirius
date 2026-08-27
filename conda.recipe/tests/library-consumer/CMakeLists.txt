cmake_minimum_required(VERSION 3.20)
project(libsirius_consumer LANGUAGES CXX)

find_package(Sirius CONFIG REQUIRED)

add_executable(libsirius_consumer main.cpp)
target_link_libraries(libsirius_consumer PRIVATE Sirius::sirius)
set_target_properties(libsirius_consumer PROPERTIES CXX_STANDARD 20)
