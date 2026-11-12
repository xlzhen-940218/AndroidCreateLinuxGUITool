//! 全局应用状态。

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::installer::{InstallState, SharedState};

/// 应用级状态：保存 adb 路径与安装任务状态。
#[derive(Clone, Default)]
pub struct AppState {
    /// 用户手动指定的 adb 可执行文件路径。
    pub adb_path: Arc<Mutex<Option<String>>>,
    pub install: SharedState,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            adb_path: Arc::new(Mutex::new(None)),
            install: Arc::new(Mutex::new(InstallState::default())),
        }
    }

    /// 脚本落地目录。
    pub fn workdir(&self) -> PathBuf {
        std::env::temp_dir().join("android-create-linux-gui-tool")
    }
}
