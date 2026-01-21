vcpkg_check_linkage(ONLY_STATIC_LIBRARY)

vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO rapidsai/kvikio
    REF branch-25.10
    SHA512 b0ad0a6cbb90560bc674be8c964a6f537ac161bf86de320009da61f9352ce7557d22550e7d85a7e410709fdcbe3e4794627d49a83b359000dfa1be574c4a3430
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_CMAKE_PATH
    REPO rapidsai/rapids-cmake
    REF v25.10.00
    SHA512 30e36f73a81a2b71137401835a322279c4f2d03d21a8c1e03126deb6a35dae51476a06cc40d0f719db2ea53db656d4bf10dfe3a9264856ae571e67c4b2b63cc2
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH BS_THREAD_POOL_PATH
    REPO bshoshany/thread-pool
    REF 097aa718f25d44315cadb80b407144ad455ee4f9
    SHA512 94177c61c5161c3cb5d088058d999239fb8bc446100e948bb9bbae44b73d0a020240c39d5232b13a628b56e233cb55a29e70baa69e511c73a6ba6a2505de1250
    HEAD_REF master
)

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}/cpp"
    OPTIONS
        -DFETCHCONTENT_SOURCE_DIR_RAPIDS-CMAKE=${RAPIDS_CMAKE_PATH}
        -DCPM_bs_thread_pool_SOURCE=${BS_THREAD_POOL_PATH}
        -DKvikIO_BUILD_EXAMPLES=OFF
        -DKvikIO_BUILD_TESTS=OFF
        -DKvikIO_BUILD_BENCHMARKS=OFF
        -DCMAKE_CUDA_ARCHITECTURES=RAPIDS
)

vcpkg_cmake_install()

# bs_thread_pool cmake config is generated but not installed
# We need to manually install it for consumers to find it
file(GLOB BS_THREAD_POOL_CMAKE_FILES
    "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/bs_thread_pool-*.cmake"
    "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/CMakeFiles/Export/*/bs_thread_pool-targets*.cmake"
)
file(INSTALL ${BS_THREAD_POOL_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/bs_thread_pool")

# Fix bs_thread_pool-targets.cmake path computation (4 dirs -> 3 dirs for share/bs_thread_pool/ layout)
execute_process(
    COMMAND sed -i "52d" "${CURRENT_PACKAGES_DIR}/share/bs_thread_pool/bs_thread_pool-targets.cmake"
)

vcpkg_cmake_config_fixup(
    PACKAGE_NAME kvikio
    CONFIG_PATH lib/cmake/kvikio
)

file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include")
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/share")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
