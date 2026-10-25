# bevy_ui_scroll

[English](README.md) · [开发指南](docs/development.zh-CN.md)

为 Bevy UI 加入平滑滚动、触摸惯性、吸附和下拉刷新。在普通 Bevy 节点上添加组件，即可通过事件控制滚动。

## 安装

使用 Rust 1.95 或更新版本，以及 Bevy 0.19.1。

```toml
[dependencies]
bevy = { version = "=0.19.1", default-features = false, features = ["ui", "bevy_ui_widgets"] }
bevy_ui_scroll = { git = "https://github.com/shengyusi-SYS/bevy_ui_scroll.git", tag = "v0.1.0" }
```

## 创建滚动面板

```rust
use bevy::prelude::*;
use bevy_ui_scroll::{EnhancedScroll, ScrollPlugin, SmoothScroll};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ScrollPlugin))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        EnhancedScroll,
        SmoothScroll,
        Node {
            width: px(360),
            height: px(400),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        children![(
            Node { height: px(1600), flex_shrink: 0.0, ..default() },
            Text::new("Scroll with the wheel or drag with a finger"),
        )],
    ));
}
```

`EnhancedScroll` 启用滚轮和触摸输入，并自动添加 `ScrollPosition`。把它用作视口的输入标记；需要 Bevy 原生滚轮处理的视口使用 `ScrollArea`。`SmoothScroll` 为按行报告的鼠标滚轮添加动画，触控板的像素输入保持直接移动。

横向滚动使用 `Overflow::scroll_x()`。嵌套的增强视口到达边界后，可将输入交给祖先视口；添加 `OverscrollBehavior::Contain` 可将输入留在当前视口。首次到达边界的事件由当前视口消费，下一次向外滚动的事件才会传给祖先。

## 定位与停止

通过 `Commands` 或 `World` 触发事件：

| 事件 | 效果 |
| --- | --- |
| `ScrollToOffset { entity, offset, behavior }` | 移动到以逻辑像素表示的 `Vec2` 偏移，方式为 `ScrollBehavior::Instant` 或 `Smooth` |
| `StopScroll { entity }` | 停止等待中的定位、滚轮动画、惯性、回弹和吸附 |
| `ScrollInput { entity, kind }` | 观察实际的滚轮、触摸或滚动条输入，包括边界处的输入 |

定位会等待布局、将目标限制在可滚动范围内，并替换之前的请求。新的用户输入会接管滚动。应用重置偏移或开始自行处理拖拽滚动时，先发送 `StopScroll`，再写入 `ScrollPosition`。停止后，下次输入或定位命令会恢复空闲吸附。

`ScrollPosition` 保存请求偏移。最近一次布局实际采用的偏移为 `ComputedNode.scroll_position * ComputedNode::inverse_scale_factor()`。触摸下拉期间，请求偏移可以越过边界，原生布局仍把内容保持在边界内。

`Scroll` 是归一化后向祖先冒泡的增量事件，包含剩余 `delta`、`original_delta`、`source` 和滚轮 `unit`。一行滚轮对应 60 个逻辑像素。取消应用中的等待定位时，观察 `ScrollInput` 最直接。

## 吸附到页面或条目

给视口添加 `ScrollSnap { axis: ScrollSnapAxis::X }`（或 `Y`），给吸附目标添加 `ScrollSnapPoint`。触发 `ScrollToSnap { entity, target: SnapTarget::Entity(point), instant: false }` 即可定位。

`SnapTarget::Index(n)` 按子节点声明顺序进行深度优先遍历，并跳过嵌套吸附子树。`ScrollSnapStatus` 分别报告选中实体和布局后是否已停稳。命令会等待视口具有非零尺寸，再使用最新几何信息解析目标。已不存在的目标会被忽略；新输入或 `StopScroll` 会取消等待。

## 下拉刷新

在 `ScrollPlugin` 之外安装 `pull_to_refresh::PullToRefreshPlugin`。将默认指示器放进包住滚动视口、已设置定位方式的外框：

```rust,ignore
use bevy_ui_scroll::pull_to_refresh::*;

commands.spawn_scene(bsn! {
    @PullToRefresh {
        @target: viewport,
        @pull_label: "下拉刷新",
        @release_label: "松开刷新",
        @refreshing_label: "正在刷新",
        @threshold: 48.0,
    }
    ChildOf(frame)
});
```

观察控件实体上的 `RefreshRequested`，开始工作时把它的 `PullToRefreshState.refreshing` 设为 `true`，完成后设为 `false`。根据控件是否处于活动状态设置 `enabled`。

阈值使用经过阻力计算后的顶部下拉距离，单位为逻辑像素。达到阈值后松手会发出一次请求；回退到阈值的 85% 时才解除触发资格。短内容同样支持下拉刷新。

自定义指示器时，创建带有 `PullRefreshTarget(viewport)` 的实体，可选添加 `PullRefreshConfig`，再读取 `PullRefreshProgress { distance, pulling, armed }` 绘制。修改默认外观时，通过 `PullToRefreshParts` 找到主体、文字、箭头和旋转指示器。弹性反馈由指示器呈现，内容继续使用原生布局。

## 滚动条与虚拟列表

Bevy 的 `Scrollbar` 和 `ScrollbarThumb` 可以用于增强视口。给触摸操作的滑块或滚动条拇指添加 `ScrollGestureCapture`，让该控件持有本次触摸手势。

长列表可配合 [bevy_ui_virtual_list](https://github.com/shengyusi-SYS/bevy_ui_virtual_list) 使用：观察其 `PooledListScrollTarget`，调用 `take_over()`，再根据提供的偏移发送 `ScrollToOffset`。将 `ScrollInput` 转为 `PooledListScrollInput`，使新输入取消等待中的索引定位。每个视口安装一个负责运动的适配器。

## 运行示例

```sh
git clone https://github.com/shengyusi-SYS/bevy_ui_scroll.git
cd bevy_ui_scroll
cargo run --example basic
```

[示例](examples/basic.rs) 包含滚动面板、原生滚动条、吸附页面和刷新指示器。Home/End 定位面板，Escape 停止，Left/Right 切换页面。在顶部下拉可启动示例中的定时刷新。构建准备、测试和源码导航见[开发指南](docs/development.zh-CN.md)。

## 许可证

原创代码采用 [MIT](LICENSE)。改编的 Android 惯性与 Chromium 速度算法保留 Apache-2.0 和 BSD-3-Clause 声明，见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
