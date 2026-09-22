set(prefix "${BUNDLE_DIR}/installed")
file(MAKE_DIRECTORY "${prefix}/lib/cmake/sirius" "${prefix}/include")
file(COPY_FILE "${BUNDLE_DIR}/libcombined.a" "${prefix}/lib/libsirius.a")
file(COPY_FILE "${BUNDLE_DIR}/libcombined.a.cmake"
     "${prefix}/lib/cmake/sirius/libsirius.a.cmake")
set(CMAKE_INSTALL_LIBDIR lib)
set(CMAKE_INSTALL_INCLUDEDIR include)
configure_file("${ROOT}/cmake/sirius-static-targets.cmake.in"
               "${prefix}/lib/cmake/sirius/sirius-static-targets.cmake" @ONLY)
set(consumer "${BUNDLE_DIR}/installed-consumer-source")
file(MAKE_DIRECTORY "${consumer}")
file(
  WRITE "${consumer}/CMakeLists.txt"
  "cmake_minimum_required(VERSION 3.30.4)\nproject(installed_static LANGUAGES CXX)\n"
  "set(PACKAGE_PREFIX_DIR \"${prefix}\")\n"
  "include(\"${prefix}/lib/cmake/sirius/sirius-static-targets.cmake\")\n"
  "add_executable(consumer \"${ROOT}/test/cmake/static_bundle/main.cpp\")\n"
  "target_link_libraries(consumer PRIVATE sirius::sirius_static)\n")
execute_process(COMMAND "${CMAKE_COMMAND}" -S "${consumer}" -B
                        "${consumer}/build" COMMAND_ERROR_IS_FATAL ANY)
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${consumer}/build"
                        COMMAND_ERROR_IS_FATAL ANY)
execute_process(COMMAND "${consumer}/build/consumer" COMMAND_ERROR_IS_FATAL ANY)
