vcpkg_check_linkage(ONLY_STATIC_LIBRARY)

vcpkg_download_distfile(
  ARCHIVE
  URLS
  "https://codeload.github.com/gcc-mirror/gcc/tar.gz/refs/tags/releases/gcc-${VERSION}"
  FILENAME
  "gcc-${VERSION}.tar.gz"
  SHA512
  56eac4f5f7b993eb2005b2ef1c2e0e0b61619c08e3afeb4c8c252bce9fa24e2efbf62b8e84259d24b22bd304e9d4a3fffd361279f682b6d5429085dab97caee7
)
vcpkg_extract_source_archive(SOURCE_PATH ARCHIVE "${ARCHIVE}")

# Extensions can be loaded after process startup; avoid a static TLS
# reservation.
vcpkg_replace_string(
  "${SOURCE_PATH}/libgomp/configure.tgt"
  "-ftls-model=initial-exec -DUSING_INITIAL_EXEC_TLS"
  "-ftls-model=global-dynamic")
vcpkg_configure_make(
  SOURCE_PATH
  "${SOURCE_PATH}"
  PROJECT_SUBPATH
  libgomp
  OPTIONS
  --with-pic
  --disable-multilib
  --disable-werror
  --disable-symvers)
vcpkg_build_make(BUILD_TARGET libgomp.la)

foreach(config release debug)
  if(NOT DEFINED VCPKG_BUILD_TYPE OR VCPKG_BUILD_TYPE STREQUAL config)
    set(suffix rel)
    set(destination lib)
    if(config STREQUAL "debug")
      set(suffix dbg)
      set(destination debug/lib)
    endif()
    file(INSTALL
         "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-${suffix}/.libs/libgomp.a"
         DESTINATION "${CURRENT_PACKAGES_DIR}/${destination}")
    file(INSTALL "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-${suffix}/omp.h"
         DESTINATION "${CURRENT_PACKAGES_DIR}/include")
  endif()
endforeach()
file(INSTALL "${CMAKE_CURRENT_LIST_DIR}/gomp-config.cmake"
     DESTINATION "${CURRENT_PACKAGES_DIR}/share/gomp")
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/COPYING3"
                        "${SOURCE_PATH}/COPYING.RUNTIME")
