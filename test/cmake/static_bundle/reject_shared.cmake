cmake_minimum_required(VERSION 3.30.4)
execute_process(
  COMMAND
    "${CMAKE_COMMAND}" -S "${SOURCE}" -B "${BINARY}" -G Ninja
    "-DCMAKE_CXX_COMPILER=${COMPILER}" -DBUNDLE_WITH_SHARED_DEP=ON
    COMMAND_ERROR_IS_FATAL ANY)
execute_process(
  COMMAND "${CMAKE_COMMAND}" --build "${BINARY}" --target combined
  RESULT_VARIABLE result
  OUTPUT_VARIABLE output
  ERROR_VARIABLE error)
if(result EQUAL 0 OR NOT "${output}${error}" MATCHES
                     "Non-static or unsupported dependency")
  message(
    FATAL_ERROR
      "Expected rejection of the shared dependency:\n${output}${error}")
endif()
