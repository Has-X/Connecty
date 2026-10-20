cmake_minimum_required(VERSION 3.24)
find_program(CONNECTY_POWERSHELL NAMES pwsh powershell REQUIRED)
execute_process(
  COMMAND "${CONNECTY_POWERSHELL}" -NoProfile -File "${CMAKE_CURRENT_SOURCE_DIR}/scripts/version.ps1"
  OUTPUT_VARIABLE CONNECTY_VERSION OUTPUT_STRIP_TRAILING_WHITESPACE
  COMMAND_ERROR_IS_FATAL ANY)
project(Connecty VERSION ${CONNECTY_VERSION} LANGUAGES CXX RC)
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
  "${CMAKE_CURRENT_SOURCE_DIR}/agent/Cargo.toml" "${CMAKE_CURRENT_SOURCE_DIR}/scripts/version.ps1")
configure_file(assets/ConnectyVersion.h.in "${CMAKE_CURRENT_BINARY_DIR}/ConnectyVersion.h" @ONLY)
if(NOT WIN32 OR NOT MSVC)
  message(FATAL_ERROR "Connecty requires Windows and the MSVC C++ toolchain.")
endif()
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
string(REPLACE "/O2" "/O1" CMAKE_CXX_FLAGS_RELEASE "${CMAKE_CXX_FLAGS_RELEASE}")
if(NOT DEFINED CONNECTY_DEPS)
  message(FATAL_ERROR "Build with scripts/build.ps1 to restore native dependencies.")
endif()
include("${CONNECTY_DEPS}/paths.cmake")
set(CONNECTY_ARCH "x64" CACHE STRING "Target architecture: x64 or arm64")
if(CONNECTY_ARCH STREQUAL "arm64")
  set(CONNECTY_MANIFEST_ARCH "arm64")
else()
  set(CONNECTY_MANIFEST_ARCH "amd64")
endif()
configure_file(assets/app.manifest "${CMAKE_CURRENT_BINARY_DIR}/Connecty.manifest" @ONLY)

add_executable(Connecty WIN32 src/App.cpp src/Core.cpp assets/Connecty.rc)
set_source_files_properties(assets/Connecty.rc PROPERTIES OBJECT_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty.ico")
target_include_directories(Connecty PRIVATE "${CMAKE_CURRENT_BINARY_DIR}" "${CONNECTY_DEPS}/generated" "${SDK_FOUNDATION}/include" "${SDK_WINUI}/include" "${SDK_INTERACTIVE}/include")
target_compile_definitions(Connecty PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)
target_compile_options(Connecty PRIVATE /W4 /EHsc /bigobj /utf-8 /permissive- $<$<CONFIG:Release>:/GL>)
target_link_directories(Connecty PRIVATE "${SDK_FOUNDATION}/lib/native/${CONNECTY_ARCH}")
target_link_libraries(Connecty PRIVATE windowsapp Microsoft.WindowsAppRuntime.Bootstrap user32 shell32 advapi32 comctl32 dwmapi ole32)
target_link_options(Connecty PRIVATE /MANIFEST:EMBED "/MANIFESTINPUT:${CMAKE_CURRENT_BINARY_DIR}/Connecty.manifest" $<$<CONFIG:Release>:/LTCG> $<$<CONFIG:Release>:/OPT:REF> $<$<CONFIG:Release>:/OPT:ICF>)
target_link_options(Connecty PRIVATE "/MAP:${CMAKE_CURRENT_BINARY_DIR}/Connecty.map")
add_custom_command(TARGET Connecty POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${SDK_FOUNDATION}/runtimes/win-${CONNECTY_ARCH}/native/Microsoft.WindowsAppRuntime.Bootstrap.dll" "$<TARGET_FILE_DIR:Connecty>"
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty.ico" "$<TARGET_FILE_DIR:Connecty>")
add_custom_command(TARGET Connecty POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty.png" "$<TARGET_FILE_DIR:Connecty>"
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty-tray.ico" "$<TARGET_FILE_DIR:Connecty>"
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty-live.ico" "$<TARGET_FILE_DIR:Connecty>"
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty-retry.ico" "$<TARGET_FILE_DIR:Connecty>"
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/assets/Connecty-error.ico" "$<TARGET_FILE_DIR:Connecty>")
enable_testing()
add_executable(CoreTests tests/CoreTests.cpp src/Core.cpp)
target_include_directories(CoreTests PRIVATE src "${CONNECTY_DEPS}/generated")
target_compile_definitions(CoreTests PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)
target_compile_options(CoreTests PRIVATE /EHsc /utf-8)
target_link_libraries(CoreTests PRIVATE windowsapp shell32 ole32 advapi32)
add_test(NAME core COMMAND CoreTests)
