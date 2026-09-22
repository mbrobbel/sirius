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

if(SIRIUS_BUILD_SHARED)
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
endif()
install(
  FILES "${CMAKE_CURRENT_SOURCE_DIR}/cmake/sirius-source-revision.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/sirius-config.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/sirius-config-version.cmake"
        "${CMAKE_CURRENT_BINARY_DIR}/sirius-duckdb-compatibility.cmake"
  DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/sirius"
  COMPONENT sirius_library)

if(SIRIUS_BUILD_STATIC)
  configure_file(
    cmake/sirius-static-targets.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/sirius-static-targets.cmake" @ONLY)
  install(
    FILES "${CMAKE_CURRENT_BINARY_DIR}/libsirius.a"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    COMPONENT sirius_library)
  install(
    FILES "${CMAKE_CURRENT_BINARY_DIR}/sirius-static-targets.cmake"
          "${CMAKE_CURRENT_BINARY_DIR}/libsirius.a.cmake"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/sirius"
    COMPONENT sirius_library)
  install(
    FILES "${CMAKE_CURRENT_BINARY_DIR}/libsirius.a.json"
    DESTINATION "${CMAKE_INSTALL_DATADIR}/sirius"
    COMPONENT sirius_library)
endif()

if(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/packaging/input-manifest.json")
  install(
    FILES "${CMAKE_CURRENT_SOURCE_DIR}/packaging/input-manifest.json"
    DESTINATION "${CMAKE_INSTALL_DATADIR}/sirius"
    COMPONENT sirius_library)
endif()
