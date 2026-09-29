cmake_minimum_required(VERSION 3.30.4)
foreach(enabled ON OFF)
  execute_process(
    COMMAND
      "${CMAKE_COMMAND}" -S "${SOURCE}" -B "${BINARY}" -G Ninja
      "-DCMAKE_CXX_COMPILER=${COMPILER}" "-DBUNDLE_EXTRA_ROOT=${enabled}"
      COMMAND_ERROR_IS_FATAL ANY)
  execute_process(COMMAND "${CMAKE_COMMAND}" --build "${BINARY}" --target
                          combined COMMAND_ERROR_IS_FATAL ANY)
  execute_process(COMMAND "${ARCHIVER}" t "${BINARY}/libcombined.a"
                  OUTPUT_VARIABLE members COMMAND_ERROR_IS_FATAL ANY)
  if(enabled AND NOT members MATCHES "extra_root")
    message(FATAL_ERROR "The extra root is missing from the archive")
  elseif(NOT enabled AND members MATCHES "extra_root")
    message(FATAL_ERROR "The removed root remains in the archive")
  endif()
endforeach()
