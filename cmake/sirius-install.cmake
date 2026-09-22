install(
  DIRECTORY include/sirius
  DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}"
  COMPONENT sirius_library)

include(CMakePackageConfigHelpers)
configure_file(
  cmake/sirius-duckdb-compatibility.cmake.in
  "${CMAKE_CURRENT_BINARY_DIR}/sirius-duckdb-compatibility.cmake" @ONLY)
configure_package_config_file(
  cmake/sirius-config.cmake.in "${CMAKE_CURRENT_BINARY_DIR}/sirius-config.cmake"
  INSTALL_DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/sirius")
write_basic_package_version_file(
  "${CMAKE_CURRENT_BINARY_DIR}/sirius-config-version.cmake"
  VERSION "${PROJECT_VERSION}"
  COMPATIBILITY SameMinorVersion)

install(
  TARGETS sirius_shared
  EXPORT sirius-targets
  LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}" COMPONENT sirius_library
  ARCHIVE DESTINATION "${CMAKE_INSTALL_LIBDIR}" COMPONENT sirius_library
  RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}" COMPONENT sirius_library)
install(
  EXPORT sirius-targets
  FILE sirius-targets.cmake
  NAMESPACE sirius::
  DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/sirius"
  COMPONENT sirius_library)
install(
  FILES "${CMAKE_CURRENT_BINARY_DIR}/sirius-config.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/sirius-config-version.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/sirius-duckdb-compatibility.cmake"
  DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/sirius"
  COMPONENT sirius_library)
