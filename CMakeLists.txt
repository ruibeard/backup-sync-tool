cmake_minimum_required(VERSION 3.20)
project(WebDavSync LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_executable(WebDavSync WIN32
    src/main.cpp
    src/app.cpp
    src/config.cpp
    src/dpapi.cpp
    src/file_scanner.cpp
    src/resource.rc
    src/sync_engine.cpp
    src/webdav_client.cpp
    src/webdav_folder_dialog.cpp
)

target_include_directories(WebDavSync PRIVATE src)
target_compile_definitions(WebDavSync PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX)
target_link_libraries(WebDavSync PRIVATE
    comctl32
    crypt32
    shell32
    shlwapi
    winhttp
)
