vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO NVIDIA/cuCollections
    REF 6d59add35767afaf8dbc03ee52f916b00cd0fb11
    SHA512 b7d022f0e677cee7896714c8fa1b93ab65bd8768a77b9b5a5711d3bdab34f68df03112a953745a74d53741e43cdd05df696a7c80eb8ec5cde6125067c1c662de
    HEAD_REF dev
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_CMAKE_PATH
    REPO rapidsai/rapids-cmake
    REF v25.10.00
    SHA512 30e36f73a81a2b71137401835a322279c4f2d03d21a8c1e03126deb6a35dae51476a06cc40d0f719db2ea53db656d4bf10dfe3a9264856ae571e67c4b2b63cc2
    HEAD_REF branch-25.10
)

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}"
    OPTIONS
        -DFETCHCONTENT_SOURCE_DIR_RAPIDS-CMAKE=${RAPIDS_CMAKE_PATH}
        -DBUILD_TESTS=OFF
        -DBUILD_BENCHMARKS=OFF
        -DBUILD_EXAMPLES=OFF
)

vcpkg_cmake_install()

vcpkg_cmake_config_fixup(CONFIG_PATH lib/cmake/cuco)

file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
