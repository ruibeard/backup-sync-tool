cmake_minimum_required(VERSION 3.20)
project(WebDavSync)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(WebDavSync WIN32
    src/main.cpp
    src/app.cpp
    src/config.cpp
    src/dpapi.cpp
    src/file_scanner.cpp
    src/sync_engine.cpp
    src/webdav_client.cpp
    src/webdav_folder_dialog.cpp
    src/resource.rc
)

target_include_directories(WebDavSync PRIVATE src)

target_compile_definitions(WebDavSync PRIVATE UNICODE _UNICODE)

target_link_libraries(WebDavSync PRIVATE
    winhttp
    crypt32
    comctl32
    shell32
    shlwapi
    ole32
    uuid
)
