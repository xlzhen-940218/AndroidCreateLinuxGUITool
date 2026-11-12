// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Release 构建是 "windows" 子系统（不显示控制台窗口），
/// 但诊断模式需要把结果输出到调用者的终端，所以这里把父进程的控制台接过来。
#[cfg(windows)]
fn attach_parent_console() {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
        fn GetStdHandle(nStdHandle: i32) -> *mut c_void;
        fn SetStdHandle(nStdHandle: i32, h: *mut c_void) -> i32;
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: *mut c_void,
        ) -> *mut c_void;
    }

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const STD_OUTPUT_HANDLE: i32 = -11;
    const STD_ERROR_HANDLE: i32 = -12;

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        let handle = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if !handle.is_null() {
            SetStdHandle(STD_OUTPUT_HANDLE, handle);
            SetStdHandle(STD_ERROR_HANDLE, handle);
        }
        // 某些环境下 stdout 已经被缓存，这里重新取一次
        let _ = GetStdHandle(STD_OUTPUT_HANDLE);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // 诊断模式：不启动 GUI，直接输出设备检测报告
    if args.iter().any(|a| a == "--diagnose" || a == "--diagnose-json") {
        attach_parent_console();
        std::process::exit(acl_tool_lib::diagnose());
    }
    // 自测模式：真机跑一遍核心链路
    if args.iter().any(|a| a == "--selftest") {
        attach_parent_console();
        std::process::exit(acl_tool_lib::selftest());
    }
    // 命令行安装模式：完整跑一遍安装流程
    if args.iter().any(|a| a == "--install") {
        attach_parent_console();
        std::process::exit(acl_tool_lib::install_cli(&args));
    }
    // 命令行：强制下载 TigerVNC Viewer（免安装版）
    if args.iter().any(|a| a == "--fetch-viewer") {
        attach_parent_console();
        std::process::exit(acl_tool_lib::fetch_viewer_cli());
    }
    // 命令行端口模式：放行端口 + adb forward + TigerVNC
    if args.iter().any(|a| a.starts_with("--port=")) {
        attach_parent_console();
        std::process::exit(acl_tool_lib::port_cli(&args));
    }
    // 命令行调试模式：在虚拟机终端里执行一条命令
    if args.iter().any(|a| a.starts_with("--exec=")) {
        attach_parent_console();
        std::process::exit(acl_tool_lib::exec_cli(&args));
    }
    acl_tool_lib::run();
}
