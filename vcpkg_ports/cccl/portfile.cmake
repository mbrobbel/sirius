vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO NVIDIA/cccl
    REF 8c04b6539859932f5602e86d38314e4d87f96420
    SHA512 7066c28e1a776a6beaf098bfaed65268e3243050cfd730c040184649dfeb3d6af636afbf1a19fc49a66f28f1b2b8d905a2918933de4900649bf965449da5c57a
    HEAD_REF main
)

# CCCL is header-only, install all headers (including extension-less C++ standard headers)
file(INSTALL "${SOURCE_PATH}/thrust/thrust/" DESTINATION "${CURRENT_PACKAGES_DIR}/include/thrust" FILES_MATCHING PATTERN "*.h" PATTERN "*.inl")
file(INSTALL "${SOURCE_PATH}/cub/cub/" DESTINATION "${CURRENT_PACKAGES_DIR}/include/cub" FILES_MATCHING PATTERN "*.cuh")
# libcudacxx has both .h/.hpp headers AND extension-less C++ standard headers (like climits, cstdint, etc.)
file(COPY "${SOURCE_PATH}/libcudacxx/include/" DESTINATION "${CURRENT_PACKAGES_DIR}/include")

# Install CMake config files
file(GLOB CCCL_CMAKE_FILES "${SOURCE_PATH}/lib/cmake/cccl/*")
file(INSTALL ${CCCL_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/cccl")
file(GLOB THRUST_CMAKE_FILES "${SOURCE_PATH}/lib/cmake/thrust/*")
file(INSTALL ${THRUST_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/thrust")
file(GLOB CUB_CMAKE_FILES "${SOURCE_PATH}/lib/cmake/cub/*")
file(INSTALL ${CUB_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/cub")
file(GLOB LIBCUDACXX_CMAKE_FILES "${SOURCE_PATH}/lib/cmake/libcudacxx/*")
file(INSTALL ${LIBCUDACXX_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/libcudacxx")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
