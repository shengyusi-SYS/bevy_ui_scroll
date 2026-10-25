# 开发指南

[English](development.md) · [使用说明](../README.zh-CN.md)

## 准备环境

安装 Rust 1.95 或更新版本，以及 Cargo 和 rustfmt。库使用官方 Bevy 0.19.1。以下命令在本仓库根目录运行。

Ubuntu 的桌面构建依赖：

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxinerama-dev
```

Windows 安装 Visual Studio C++ 构建工具和 Windows SDK。macOS 通过 `xcode-select --install` 安装 Xcode 命令行工具。运行示例需要桌面会话和 Bevy 支持的显卡驱动。

## 构建与检查

```sh
cargo fmt --all -- --check
cargo check --all-targets
cargo test --lib
cargo doc --no-deps
cargo run --example basic
```

自动化测试在内存中验证几何、ECS 系统和布局；输入与外观通过窗口示例检查。生成的 API 文档位于 `target/doc/bevy_ui_scroll/index.html`。CI 使用 Linux 和稳定版 Rust，执行格式、编译、单元测试及 API 文档检查。

## 源码导航

| 文件 | 职责 |
| --- | --- |
| `src/input.rs`、`src/event/` | 选择视口、分发滚轮和触摸输入、处理嵌套手势交接 |
| `src/navigation.rs`、`src/motion.rs` | 偏移命令、取消和共用弹簧计算 |
| `src/fling.rs`、`src/velocity.rs`、`src/overscroll.rs` | 触摸速度、惯性和边界回弹 |
| `src/snap.rs`、`src/snap/` | 吸附目标、选中与停稳状态 |
| `src/gesture_capture.rs` | 控件的触摸所有权 |
| `src/pull_to_refresh.rs` | 刷新手势、外部控制的状态和默认指示器 |
| `examples/basic.rs` | 可运行的交互演示 |

输入先确定视口并停止它之前的运动。Update 推进当前运动；PostUpdate 在 `UiSystems::Layout` 之后解析吸附请求并观察结果。`RequestRedraw` 让响应式窗口中的运动持续推进。刷新工作由调用方开始和完成。

修改输入或运动时，在示例中检查行滚轮与像素输入、触摸释放和取消、嵌套边界、滚动条接管以及调整窗口后的吸附。修改刷新时，检查短拉、越过阈值、回退、释放和完成。示例状态文字可以对照请求偏移与实际偏移。

## 修改流程

从受影响的公开组件或事件出发，追踪到消费它的系统。行为变化增加有针对性的回归测试。库的数据使用 Bevy 实体、组件和几何信息表达，应用内容通过公开事件和绑定入口接入。

公开 API 或准备步骤变化时，同步更新中英文 README。开发步骤集中在本指南，API 细节放在公开类型旁。提交 pull request 前运行上面的检查，并说明用户可见的结果和验证方式。

## 发布

更新包版本及两份 README 安装示例中的 Git tag。完成检查并提交后，在该提交上创建 `vX.Y.Z` 标签，推送提交和标签，再创建含简短中英文说明的 GitHub release。发布内容保留随库提供的许可证文件。
