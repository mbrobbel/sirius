function(sirius_source_revision source_dir output)
  if(EXISTS "${source_dir}/.sirius-revision")
    file(READ "${source_dir}/.sirius-revision" revision)
    string(STRIP "${revision}" revision)
  else()
    find_package(Git REQUIRED)
    execute_process(
      COMMAND "${GIT_EXECUTABLE}" -C "${source_dir}" rev-parse HEAD
      OUTPUT_VARIABLE revision
      OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
  endif()
  string(LENGTH "${revision}" length)
  if(NOT length EQUAL 40 OR NOT revision MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "Invalid recorded source revision in ${source_dir}")
  endif()
  set(${output}
      "${revision}"
      PARENT_SCOPE)
endfunction()
