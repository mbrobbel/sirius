vcpkg_check_linkage(ONLY_STATIC_LIBRARY)

set(NVCOMP_VERSION "5.0.0.6")
set(CUDA_VERSION "12")

vcpkg_download_distfile(ARCHIVE
    URLS "https://developer.download.nvidia.com/compute/nvcomp/redist/nvcomp/linux-x86_64/nvcomp-linux-x86_64-${NVCOMP_VERSION}_cuda${CUDA_VERSION}-archive.tar.xz"
    FILENAME "nvcomp-linux-x86_64-${NVCOMP_VERSION}_cuda${CUDA_VERSION}-archive.tar.xz"
    SHA512 4a941b3498c09971cd6751a9fd4709f290b66c82c328d73624aa2cf7c3fadac2311413b5e064599a314baa1ed0f6827f96882fff46723efd1e1932fb19785153
)

vcpkg_extract_source_archive(
    SOURCE_PATH
    ARCHIVE "${ARCHIVE}"
)

# Install headers
file(GLOB HEADER_FILES "${SOURCE_PATH}/include/*")
file(INSTALL ${HEADER_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/include")

# Install libraries
file(GLOB LIB_FILES "${SOURCE_PATH}/lib/*.a")
file(INSTALL ${LIB_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/lib")

# Install CMake config files (targets only, we'll write a custom config.cmake)
file(INSTALL "${SOURCE_PATH}/lib/cmake/nvcomp/nvcomp-config-version.cmake" DESTINATION "${CURRENT_PACKAGES_DIR}/share/nvcomp")
file(INSTALL "${SOURCE_PATH}/lib/cmake/nvcomp/nvcomp-targets-static.cmake" DESTINATION "${CURRENT_PACKAGES_DIR}/share/nvcomp")
file(INSTALL "${SOURCE_PATH}/lib/cmake/nvcomp/nvcomp-targets-static-release.cmake" DESTINATION "${CURRENT_PACKAGES_DIR}/share/nvcomp")

# Write a custom config file that works with vcpkg layout
file(WRITE "${CURRENT_PACKAGES_DIR}/share/nvcomp/nvcomp-config.cmake" "
get_filename_component(PACKAGE_PREFIX_DIR \"\${CMAKE_CURRENT_LIST_DIR}/../../\" ABSOLUTE)

set(nvcomp_VERSION 5.0.0.6)
set(nvcomp_INCLUDE_DIR \"\${PACKAGE_PREFIX_DIR}/include\")
set(nvcomp_LIBRARY_DIR \"\${PACKAGE_PREFIX_DIR}/lib\")

# Check headers and library directories exist
if(NOT EXISTS \"\${nvcomp_INCLUDE_DIR}/nvcomp.h\")
    message(FATAL_ERROR \"nvcomp headers not found at \${nvcomp_INCLUDE_DIR}\")
endif()

# Load the target definitions
include(\"\${CMAKE_CURRENT_LIST_DIR}/nvcomp-targets-static.cmake\")

# Create alias for compatibility with downstream projects
if(TARGET nvcomp::nvcomp_static AND NOT TARGET nvcomp::nvcomp)
    add_library(nvcomp::nvcomp ALIAS nvcomp::nvcomp_static)
endif()

set(nvcomp_FOUND TRUE)
")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
