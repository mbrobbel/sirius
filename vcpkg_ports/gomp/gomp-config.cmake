include(CMakeFindDependencyMacro)
find_dependency(Threads)
if(NOT TARGET gomp::gomp)
  add_library(gomp::gomp STATIC IMPORTED)
  set_target_properties(
    gomp::gomp
    PROPERTIES IMPORTED_LOCATION "${CMAKE_CURRENT_LIST_DIR}/../../lib/libgomp.a"
               INTERFACE_LINK_LIBRARIES "Threads::Threads;${CMAKE_DL_LIBS}")
  if(EXISTS "${CMAKE_CURRENT_LIST_DIR}/../../debug/lib/libgomp.a")
    set_target_properties(
      gomp::gomp
      PROPERTIES IMPORTED_LOCATION_DEBUG
                 "${CMAKE_CURRENT_LIST_DIR}/../../debug/lib/libgomp.a")
  endif()
endif()
