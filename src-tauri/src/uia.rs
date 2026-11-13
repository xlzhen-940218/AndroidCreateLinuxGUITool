//! 基于 `uiautomator dump` 的界面自动化。
//!
//! 只依赖无障碍节点树（而不是 OCR），因此在中文/英文系统下都能用
//! `resource-id` 精确定位控件，避免"注意中英文区别"带来的匹配问题。

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

use crate::adb::{input_text_payload, sh_quote, Adb};

/// 界面节点矩形（设备像素坐标）。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
}

impl Rect {
    pub fn center(&self) -> (i64, i64) {
        ((self.x1 + self.x2) / 2, (self.y1 + self.y2) / 2)
    }
    pub fn height(&self) -> i64 {
        self.y2 - self.y1
    }
}

/// 一个可交互节点。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNode {
    pub text: String,
    pub desc: String,
    pub id: String,
    pub class: String,
    pub package: String,
    pub clickable: bool,
    pub checkable: bool,
    pub checked: bool,
    pub enabled: bool,
    pub focusable: bool,
    pub focused: bool,
    pub selected: bool,
    pub scrollable: bool,
    pub long_clickable: bool,
    pub bounds: Rect,
}

impl UiNode {
    /// 文本/描述/类名里是否包含关键词（忽略大小写）。
    pub fn mentions(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.text.to_lowercase().contains(&needle)
            || self.desc.to_lowercase().contains(&needle)
            || self.class.to_lowercase().contains(&needle)
            || self.id.to_lowercase().contains(&needle)
    }
}

/// 解析 `[left,top][right,bottom]` 形式的 bounds。
fn parse_bounds(raw: &str) -> Rect {
    // 格式是 `[left,top][right,bottom]`：不能按逗号切分（中间那段的
    // `181][1012` 解析不出来），直接按非数字字符切。
    let nums: Vec<i64> = raw
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    if nums.len() >= 4 {
        Rect {
            x1: nums[0],
            y1: nums[1],
            x2: nums[2],
            y2: nums[3],
        }
    } else {
        Rect {
            x1: 0,
            y1: 0,
            x2: 0,
            y2: 0,
        }
    }
}

/// 转储当前界面的节点树。
pub async fn dump(adb: &Adb) -> Result<Vec<UiNode>> {
    let mut last_err = String::new();
    for _ in 0..3 {
        let out = adb
            .raw(&[
                "shell",
                "uiautomator dump --compressed /sdcard/acl_window_dump.xml",
            ])
            .await?;
        let text = out.combined();
        if text.contains("ERROR") {
            last_err = text.trim().to_string();
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            continue;
        }
        let xml = adb.shell("cat /sdcard/acl_window_dump.xml").await?;
        if xml.trim().is_empty() {
            last_err = "dump 文件为空".to_string();
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            continue;
        }
        return Ok(parse_nodes(&xml));
    }
    Err(anyhow!("无法获取界面节点树: {}", last_err))
}

/// 解析 uiautomator 的 XML 输出。
pub fn parse_nodes(xml: &str) -> Vec<UiNode> {
    let mut nodes = Vec::new();
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return nodes,
    };
    for node in doc.descendants().filter(|n| n.has_tag_name("node")) {
        let attr = |name: &str| {
            node.attribute(name)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let flag = |name: &str| attr(name).eq_ignore_ascii_case("true");
        nodes.push(UiNode {
            text: attr("text"),
            desc: attr("content-desc"),
            id: attr("resource-id"),
            class: attr("class"),
            package: attr("package"),
            clickable: flag("clickable"),
            checkable: flag("checkable"),
            checked: flag("checked"),
            enabled: attr("enabled").eq_ignore_ascii_case("true"),
            focusable: flag("focusable"),
            focused: flag("focused"),
            selected: flag("selected"),
            scrollable: flag("scrollable"),
            long_clickable: flag("long-clickable"),
            bounds: parse_bounds(&attr("bounds")),
        });
    }
    nodes
}

/// 按 resource-id 尾缀查找节点（忽略包名前缀）。
pub fn find_by_id<'a>(nodes: &'a [UiNode], id_suffix: &str) -> Option<&'a UiNode> {
    nodes.iter().find(|n| {
        n.id == id_suffix || n.id.ends_with(&format!(":id/{}", id_suffix))
    })
}

/// 按文本精确匹配查找可点击节点。
pub fn find_by_text<'a>(nodes: &'a [UiNode], text: &str) -> Option<&'a UiNode> {
    nodes.iter().find(|n| n.text.trim() == text)
}

/// 节点树里是否存在处于焦点状态的文本输入框。
///
/// xterm.js 会用一个隐藏的 `<textarea>` 接收输入法输入，
/// 在无障碍节点树里表现为 `android.widget.EditText`（content-desc="Cursor"）。
/// 只有它拿到焦点，键入的文字才会真正进入终端——这一点比
/// 「输入法服务的 served view 是否是终端」更能反映真实可用性。
pub fn has_focused_text_input(nodes: &[UiNode]) -> Option<bool> {
    let has_edit = nodes.iter().any(|n| n.class.contains("EditText"));
    if !has_edit {
        // 节点树里根本没有输入框节点（某些无障碍状态下会这样），交给上层判断
        return None;
    }
    Some(nodes.iter().any(|n| n.focused && n.class.contains("EditText")))
}

/// 点击坐标。
pub async fn tap_xy(adb: &Adb, x: i64, y: i64) -> Result<()> {
    adb.raw(&["shell", &format!("input tap {} {}", x, y)])
        .await
        .map(|_| ())
}

/// 点击节点中心。
pub async fn tap_node(adb: &Adb, node: &UiNode) -> Result<()> {
    let (x, y) = node.bounds.center();
    if node.bounds.x2 <= node.bounds.x1 && node.bounds.y2 <= node.bounds.y1 {
        // 点 (0,0) 会落到状态栏/通知栏上，宁可直接报错
        bail!(
            "控件「{}」没有有效的坐标（bounds 为 0），无法点击。",
            if node.text.is_empty() { &node.desc } else { &node.text }
        );
    }
    tap_xy(adb, x, y).await
}

/// 输入文本（空格转 `%s`，整串用单引号保护，避免被设备 shell 拆开）。
pub async fn type_text(adb: &Adb, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let payload = input_text_payload(text);
    let cmd = format!("input text {}", sh_quote(&payload));
    let out = adb.raw(&["shell", &cmd]).await?;
    if !out.ok() {
        bail!("输入文本失败: {}", out.combined().trim());
    }
    Ok(())
}

/// 发送按键。
pub async fn key(adb: &Adb, keycode: &str) -> Result<()> {
    adb.raw(&["shell", &format!("input keyevent {}", keycode)])
        .await
        .map(|_| ())
}

/// 回车。
pub async fn press_enter(adb: &Adb) -> Result<()> {
    key(adb, "66").await
}

/// 滑动（上滑 = 内容向下翻页）。
pub async fn swipe(adb: &Adb, x1: i64, y1: i64, x2: i64, y2: i64, ms: i64) -> Result<()> {
    adb.raw(&["shell", &format!("input swipe {} {} {} {} {}", x1, y1, x2, y2, ms)])
        .await
        .map(|_| ())
}

/// 读取当前前台 Activity / 包名，用于判断界面属于哪个 App。
pub async fn foreground_package(adb: &Adb) -> Result<String> {
    let out = adb
        .shell("dumpsys window 2>/dev/null | grep -E 'mCurrentFocus|mFocusedApp'")
        .await
        .unwrap_or_default();
    for token in out.split_whitespace() {
        if token.contains('/') && token.contains('.') {
            let pkg = token.split('/').next().unwrap_or("");
            if !pkg.is_empty() && pkg != "Window" {
                return Ok(pkg.to_string());
            }
        }
    }
    Ok(String::new())
}

/// 屏幕尺寸（像素）。
pub async fn display_size(adb: &Adb) -> Result<(i64, i64)> {
    let out = adb.shell("wm size").await.unwrap_or_default();
    // "Physical size: 1080x2400" / "Override size: ..."
    let line = out
        .lines()
        .find(|l| l.contains("size:"))
        .unwrap_or_default();
    let dims = line
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut it = dims.split('x');
    let w = it.next().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(1080);
    let h = it.next().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(2400);
    Ok((w, h))
}

/// 向下滚动列表（手指从下往上滑）。
pub async fn scroll_down(adb: &Adb) -> Result<()> {
    let (w, h) = display_size(adb).await?;
    swipe(adb, w / 2, (h as f64 * 0.72) as i64, w / 2, (h as f64 * 0.28) as i64, 320).await
}
