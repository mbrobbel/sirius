cmake_minimum_required(VERSION 3.30.4)
project(sirius_static_openmp_test LANGUAGES CXX)
include(CTest)
find_package(gomp CONFIG REQUIRED)
find_package(OpenMP REQUIRED COMPONENTS CXX)
add_library(parallel SHARED parallel.cpp)
target_link_libraries(
  parallel PRIVATE "$<COMPILE_ONLY:OpenMP::OpenMP_CXX>"
                   "$<LINK_LIBRARY:WHOLE_ARCHIVE,gomp::gomp>")
add_executable(loader loader.cpp)
target_link_libraries(loader PRIVATE ${CMAKE_DL_LIBS})
add_test(NAME bundled_openmp COMMAND loader "$<TARGET_FILE:parallel>")
set_tests_properties(bundled_openmp PROPERTIES ENVIRONMENT "OMP_NUM_THREADS=4"
                                               TIMEOUT 30)
