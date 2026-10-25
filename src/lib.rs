//! 滚动系统
//!
//! 提供显式 opt-in 的鼠标滚轮、触屏滑动、程序定位和原生滚动条接管。
//! 包含 scroll-snap 支持：触摸惯性滚动 + 自动吸附到 snap 点。
//!
//! 模块结构：
//! - `event` — 事件注入/处理、方向锁定、Scroll Session、smooth scroll
//! - `fling` — Android OverScroller 移植的惯性滑行
//! - `overscroll` — 普通容器 overscroll 回弹 spring
//! - `velocity` — Chrome LSQ2 速度采样
//! - `snap` — Scroll-snap 触摸追踪和 spring 动画
//! - `pull_to_refresh` — optional refresh behavior and default indicator

mod event;
mod input;
mod motion;
mod navigation;
pub use navigation::{ScrollBehavior, ScrollToOffset};
mod fling;
mod gesture_capture;
#[cfg(test)]
mod input_tests;
mod overscroll;
pub mod pull_to_refresh;
pub mod snap;
mod velocity;

pub use event::{OverscrollBehavior, Scroll, ScrollSource, SmoothScroll};
pub use gesture_capture::ScrollGestureCapture;
pub use input::{ScrollInput, ScrollInputKind};
pub(crate) use overscroll::release_refresh_pull;
pub use snap::{ScrollSnap, ScrollSnapAxis, ScrollSnapPoint, ScrollSnapStatus, ScrollToSnap, SnapTarget};

use bevy::{
    app::{App, Plugin, Update},
    ecs::{entity::Entity, event::Event},
    prelude::{ApplyDeferred, IntoScheduleConfigs, On},
};

/// Native layout's scrollable range, in logical pixels. Reserved scrollbars
/// reduce the viewport; every input/animation path must use the same boundary.
fn max_scroll_offset(computed: &bevy::ui::ComputedNode) -> bevy::math::Vec2 {
    ((computed.content_size() - computed.size() + computed.scrollbar_size) * computed.inverse_scale_factor())
        .max(bevy::math::Vec2::ZERO)
}

/// 主动停止某个滚动实体上的所有滚动动画/惯性。
///
/// 用于“先停滚动，再执行程序化滚动”的场景（如点击定位按钮）。
#[derive(Event)]
pub struct StopScroll {
    pub entity: Entity,
}

fn on_stop_scroll(ev: On<StopScroll>, mut commands: bevy::prelude::Commands) {
    let entity = ev.entity;
    commands.queue(move |world: &mut bevy::prelude::World| navigation::stop_motion(world, entity));
}

pub struct ScrollPlugin;
impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<bevy::window::RequestRedraw>()
            .init_resource::<event::TouchScrollState>()
            .init_resource::<gesture_capture::TouchGestureCaptureState>()
            .init_resource::<gesture_capture::PendingTouchGestureCaptureReleases>()
            .init_resource::<fling::FlingState>()
            .init_resource::<overscroll::OverscrollBounceState>()
            .add_systems(
                Update,
                (ApplyDeferred, gesture_capture::cleanup_removed_gesture_capture)
                    .chain()
                    .before(event::send_scroll_events)
                    .before(snap::scroll_snap_touch_tracker),
            )
            .add_systems(
                Update,
                (
                    event::send_scroll_events,
                    navigation::animate_offsets,
                    fling::touch_fling_system,
                    event::smooth_scroll_system,
                    overscroll::overscroll_bounce_system,
                    snap::scroll_snap_touch_tracker,
                    snap::scroll_snap_system,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    gesture_capture::release_ended_touch_captures,
                    gesture_capture::flush_pending_touch_releases,
                )
                    .chain()
                    .after(snap::scroll_snap_touch_tracker),
            )
            .add_observer(gesture_capture::capture_touch_press)
            .add_observer(gesture_capture::release_touch_release)
            .add_observer(gesture_capture::release_touch_drag_end)
            .add_observer(gesture_capture::release_touch_cancel)
            .add_observer(navigation::on_scroll_to_offset)
            .add_observer(navigation::on_scroll_input)
            .add_systems(
                bevy::prelude::PostUpdate,
                (
                    snap::resolve_snap_requests,
                    snap::observe_snap_status,
                    navigation::wake_ready_offsets,
                )
                    .chain()
                    .after(bevy::ui::UiSystems::Layout),
            )
            .add_observer(input::on_pointer_wheel)
            .add_observer(input::on_scrollbar_press)
            .add_observer(event::on_scroll_handler)
            .add_observer(snap::on_scroll_to_snap)
            .add_observer(on_stop_scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{app::App, math::Vec2, prelude::*};

    #[test]
    fn stop_scroll_clears_fling_for_target() {
        let mut app = App::new();
        app.add_plugins(ScrollPlugin);

        let target = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<fling::FlingState>()
            .entries
            .push(fling::FlingEntry {
                target,
                total_distance: Vec2::ZERO,
                duration: Vec2::ZERO,
                elapsed: 0.0,
                prev_position_fraction: Vec2::ZERO,
            });
        let other = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<fling::FlingState>()
            .entries
            .push(fling::FlingEntry {
                target: other,
                total_distance: Vec2::ZERO,
                duration: Vec2::ZERO,
                elapsed: 0.0,
                prev_position_fraction: Vec2::ZERO,
            });

        app.world_mut().trigger(StopScroll { entity: target });
        app.world_mut().flush();

        let remaining: Vec<_> = app
            .world()
            .resource::<fling::FlingState>()
            .entries
            .iter()
            .map(|entry| entry.target)
            .collect();
        assert_eq!(remaining, vec![other]);
    }
}

/// Opts a native overflow viewport into enhanced wheel/touch handling.
/// Use instead of Bevy's `ScrollArea` on this viewport; native descendants stay native.
#[derive(bevy::prelude::Component, Default, Clone, Copy)]
#[require(bevy::ui::ScrollPosition)]
pub struct EnhancedScroll;
