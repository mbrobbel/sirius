vcpkg_check_linkage(ONLY_STATIC_LIBRARY)

vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO rapidsai/cudf
    REF "v${VERSION}0"
    SHA512 4364a681327e84946238e1208b75e79113c7783a0b22eec2b42f34382e98b3001225678c37a84035f591ff2ddca7c8a802ead87cf97d0fdf2e9a9783c9233df4
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_CMAKE_PATH
    REPO rapidsai/rapids-cmake
    REF "v${VERSION}0"
    SHA512 30e36f73a81a2b71137401835a322279c4f2d03d21a8c1e03126deb6a35dae51476a06cc40d0f719db2ea53db656d4bf10dfe3a9264856ae571e67c4b2b63cc2
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_LOGGER_PATH
    REPO rapidsai/rapids-logger
    REF 46070bb255482f0782ca840ae45de9354380e298
    SHA512 f9ac098cce0339c0a958e5a476b2a7461bb68ef1fe702d7b1bd2a427a0decf2b5e87ac195297933fa6c838c9cebfdc0bc8de3f79df75a1baa9c0e85f03ce51ae
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH JITIFY_PATH
    REPO NVIDIA/jitify
    REF 44e978b21fc8bdb6b2d7d8d179523c8350db72e5
    SHA512 c6a175ae6ebae066285f1d662f8a7f73ea595fa17cf1ae7c66261899f5458e0c674eb5d546c404b8840cd1a2e760d72903b7bf6f5a48d32b13ebb5325256a2c4
    HEAD_REF master
)

# Note: dlpack is provided by vcpkg dependency
# bs_thread_pool is not in vcpkg, so we fetch it manually
vcpkg_from_github(
    OUT_SOURCE_PATH BS_THREAD_POOL_PATH
    REPO bshoshany/thread-pool
    REF 097aa718f25d44315cadb80b407144ad455ee4f9
    SHA512 94177c61c5161c3cb5d088058d999239fb8bc446100e948bb9bbae44b73d0a020240c39d5232b13a628b56e233cb55a29e70baa69e511c73a6ba6a2505de1250
    HEAD_REF master
)

# Patch cudf to use nanoarrow 0.7.0 instead of 0.7.0.dev (which vcpkg rejects)
vcpkg_replace_string("${SOURCE_PATH}/cpp/cmake/thirdparty/get_nanoarrow.cmake"
    "0.7.0.dev"
    "0.7.0"
)

# Patch dlpack - vcpkg's 0.8 port incorrectly reports version 0.6, so use 0.6
vcpkg_replace_string("${SOURCE_PATH}/cpp/cmake/thirdparty/get_dlpack.cmake"
    "find_and_configure_dlpack(\${CUDF_MIN_VERSION_dlpack})"
    "find_and_configure_dlpack(\"0.6\")"
)

# Patch rapids_logger to use vcpkg's spdlog::spdlog target instead of spdlog
vcpkg_replace_string("${RAPIDS_LOGGER_PATH}/CMakeLists.txt"
    "set_target_properties(spdlog PROPERTIES POSITION_INDEPENDENT_CODE ON)"
    "set_target_properties(spdlog::spdlog PROPERTIES POSITION_INDEPENDENT_CODE ON)"
)

# Patch nanoarrow - vcpkg's nanoarrow is an ALIAS target, can't set properties on it
vcpkg_replace_string("${SOURCE_PATH}/cpp/cmake/thirdparty/get_nanoarrow.cmake"
    "set_target_properties(nanoarrow PROPERTIES POSITION_INDEPENDENT_CODE ON)"
    "# set_target_properties disabled for vcpkg ALIAS target"
)

# Patch decompression.cpp to define ZSTD_STATIC_LINKING_ONLY before including zstd.h
# This exposes ZSTD_findDecompressedSize which is needed by cudf but only available
# when statically linking zstd. vcpkg provides static zstd but doesn't set this define.
vcpkg_replace_string("${SOURCE_PATH}/cpp/src/io/comp/decompression.cpp"
    "#include <zstd.h>"
    "#define ZSTD_STATIC_LINKING_ONLY
#include <zstd.h>"
)

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}/cpp"
    OPTIONS
        -DFETCHCONTENT_SOURCE_DIR_RAPIDS-CMAKE=${RAPIDS_CMAKE_PATH}
        -DCPM_rapids_logger_SOURCE=${RAPIDS_LOGGER_PATH}
        -DCPM_jitify_SOURCE=${JITIFY_PATH}
        -DCPM_bs_thread_pool_SOURCE=${BS_THREAD_POOL_PATH}
        -DCMAKE_CUDA_ARCHITECTURES=RAPIDS
        -DBUILD_SHARED_LIBS=OFF
        -DCUDA_STATIC_RUNTIME=ON
        -DBUILD_TESTS=OFF
        -DBUILD_BENCHMARKS=OFF
        -DCMAKE_C_COMPILER_LAUNCHER=sccache
        -DCMAKE_CXX_COMPILER_LAUNCHER=sccache
        -DCMAKE_CUDA_COMPILER_LAUNCHER=sccache
)

vcpkg_cmake_install()
vcpkg_cmake_config_fixup(
    PACKAGE_NAME cudf
    CONFIG_PATH lib/cmake/cudf
)

file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include")
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/share")

# Fix cudf-dependencies.cmake to skip ALIAS targets when setting IMPORTED_GLOBAL
# ALIAS targets like CCCL::CUB, CCCL::libcudacxx don't support set_target_properties
vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/share/cudf/cudf-dependencies.cmake"
[[foreach(target IN LISTS rapids_global_targets)
  if(TARGET ${target})
    get_target_property(_is_imported ${target} IMPORTED)
    get_target_property(_already_global ${target} IMPORTED_GLOBAL)
    if(_is_imported AND NOT _already_global)
        set_target_properties(${target} PROPERTIES IMPORTED_GLOBAL TRUE)
    endif()
  endif()
endforeach()]]
[[foreach(target IN LISTS rapids_global_targets)
  if(TARGET ${target})
    get_target_property(_aliased ${target} ALIASED_TARGET)
    if(_aliased)
      # Skip ALIAS targets - can't set properties on them
      continue()
    endif()
    get_target_property(_is_imported ${target} IMPORTED)
    get_target_property(_already_global ${target} IMPORTED_GLOBAL)
    if(_is_imported AND NOT _already_global)
        set_target_properties(${target} PROPERTIES IMPORTED_GLOBAL TRUE)
    endif()
  endif()
endforeach()]]
)

# Remove rapids_logger files that conflict with rmm (rmm already provides them)
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/include/rapids_logger")
file(REMOVE "${CURRENT_PACKAGES_DIR}/lib/librapids_logger.a")
file(REMOVE "${CURRENT_PACKAGES_DIR}/debug/lib/librapids_logger.a")

# Fix cudf-targets.cmake to use proper vcpkg target names for nanoarrow and zstd
# cudf uses "nanoarrow" and "zstd" but vcpkg provides "nanoarrow::nanoarrow" and "zstd::libzstd_static"
vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/share/cudf/cudf-targets.cmake"
    "\$<LINK_ONLY:nanoarrow>"
    "\$<LINK_ONLY:nanoarrow::nanoarrow>"
)
vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/share/cudf/cudf-targets.cmake"
    "\$<LINK_ONLY:zstd>"
    "\$<LINK_ONLY:zstd::libzstd_static>"
)

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")

# # Create usage file
# file(WRITE "${CURRENT_PACKAGES_DIR}/share/${PORT}/usage" "\
# cudf provides CMake targets:

#     find_package(cudf CONFIG REQUIRED)
#     target_link_libraries(main PRIVATE cudf::cudf)

# Requirements:
# - CUDA 12.2+ with Compute Capability 7.0+ GPU (Volta or newer)
# - Statically linked with Arrow, RMM, and CUDA runtime
# - Supports Parquet, ORC, JSON file formats

# Note: This is a static build of libcudf.
# For Python bindings, use conda/pip installation instead.
# ")
