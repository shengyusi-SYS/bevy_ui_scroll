//! 滚动事件注入和处理。

mod delta;
mod touch;

pub(crate) use touch::entity_depth;

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        event::EntityEvent,
        observer::On,
        query::{With, Without},
        system::{Commands, Query, ResMut},
    },
    math::Vec2,
    ui::{ComputedNode, Node, OverflowAxis, ScrollPosition},
};

use super::overscroll::OverscrollBounceState;
use super::snap::{ScrollSnap, ScrollSnapAxis, ScrollSnapState};

// Re-exports for parent (scroll.rs) and sibling modules
pub use delta::SmoothScroll;
pub(crate) use delta::SmoothScrollState;
pub(super) use delta::{apply_snap_delta, smooth_scroll_system};
pub(super) use touch::{TouchScrollState, send_scroll_events};

use delta::apply_normal_or_smooth_delta;

/// Overscroll 行为控制
#[derive(Clone, Copy, PartialEq, Eq, Default, Component)]
pub enum OverscrollBehavior {
    #[default]
    Auto,
    Contain,
}

// ── 方向锁定 ──

/// 触摸方向锁定 (Chrome "rail" 概念)
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectionLock {
    /// 尚未确定（在 Touch Slop 内）
    None,
    /// 锁定水平轴
    Horizontal,
    /// 锁定垂直轴
    Vertical,
    /// 自由滚动（对角线滑动，两轴都不满足 2:1 比例）
    Free,
}

// ── Scroll 事件 ──

/// 滚动事件来源
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollSource {
    Wheel,
    Touch,
}

/// 可冒泡的滚动事件。
#[derive(EntityEvent, Debug)]
#[entity_event(propagate, auto_propagate)]
pub struct Scroll {
    pub entity: Entity,
    pub delta: Vec2,
    pub source: ScrollSource,
    pub original_delta: Vec2,
    /// Original input units; `delta` is normalized to logical pixels.
    pub unit: bevy::input::mouse::MouseScrollUnit,
}

// ── on_scroll_handler ──

/// 处理 Scroll 事件
pub(super) fn on_scroll_handler(
    mut scroll: On<Scroll>,
    mut query: Query<
        (
            &mut ScrollPosition,
            &Node,
            &ComputedNode,
            Option<&ScrollSnap>,
            Option<&mut ScrollSnapState>,
            Option<&OverscrollBehavior>,
            Option<&mut SmoothScrollState>,
        ),
        (With<crate::EnhancedScroll>, Without<bevy::ui_widgets::ScrollArea>),
    >,
    mut commands: Commands,
    mut bounce_state: ResMut<OverscrollBounceState>,
) {
    let Ok((mut scroll_position, node, computed, snap_opt, mut state_opt, overscroll_opt, mut smooth_opt)) =
        query.get_mut(scroll.entity)
    else {
        if scroll.source == ScrollSource::Touch {
            bevy::log::trace!(
                "[scroll] query MISS for entity {:?}, delta={:?}",
                scroll.entity,
                scroll.delta
            );
        }
        return;
    };

    commands.trigger(crate::ScrollInput {
        entity: scroll.entity,
        kind: match scroll.source {
            ScrollSource::Wheel => crate::ScrollInputKind::Wheel,
            ScrollSource::Touch => crate::ScrollInputKind::Touch,
        },
    });
    // Pixel deltas already carry a device/OS motion profile.
    if scroll.unit == bevy::input::mouse::MouseScrollUnit::Pixel {
        if let Some(smooth) = smooth_opt.as_deref_mut() {
            *smooth = SmoothScrollState::default();
        }
        smooth_opt = None;
    }
    let max_offset = crate::max_scroll_offset(computed);
    if scroll.source == ScrollSource::Touch {
        bevy::log::trace!(
            "[scroll] HIT entity {:?}, pos={:?} max_offset={max_offset:?} delta={:?} overflow=({:?},{:?})",
            scroll.entity,
            scroll_position.0,
            scroll.delta,
            node.overflow.x,
            node.overflow.y
        );
    }
    let source = scroll.source;
    let scroll_entity = scroll.entity;
    let original_delta = scroll.original_delta;
    let contain = overscroll_opt.copied().unwrap_or_default() == OverscrollBehavior::Contain;
    let has_overscroll = source == ScrollSource::Touch && snap_opt.is_none();
    let delta = &mut scroll.delta;

    if node.overflow.x == OverflowAxis::Scroll && delta.x != 0.0 {
        if let Some(snap_cfg) = snap_opt {
            if snap_cfg.axis == ScrollSnapAxis::X {
                if source == ScrollSource::Wheel && original_delta.y.abs() > original_delta.x.abs() {
                    // 跳过 X-axis snap
                } else {
                    apply_snap_delta(
                        &mut scroll_position.x,
                        delta.x,
                        max_offset.x.max(0.0),
                        source,
                        state_opt.as_deref_mut(),
                    );
                    delta.x = 0.0;
                }
            } else {
                apply_normal_or_smooth_delta(
                    &mut scroll_position.x,
                    &mut delta.x,
                    max_offset.x,
                    contain,
                    source,
                    smooth_opt.as_deref_mut(),
                    true,
                    has_overscroll,
                );
            }
        } else {
            apply_normal_or_smooth_delta(
                &mut scroll_position.x,
                &mut delta.x,
                max_offset.x,
                contain,
                source,
                smooth_opt.as_deref_mut(),
                true,
                has_overscroll,
            );
        }
    }

    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0.0 {
        if let Some(snap_cfg) = snap_opt {
            if snap_cfg.axis == ScrollSnapAxis::Y {
                apply_snap_delta(
                    &mut scroll_position.y,
                    delta.y,
                    max_offset.y.max(0.0),
                    source,
                    state_opt.as_deref_mut(),
                );
                delta.y = 0.0;
            } else {
                apply_normal_or_smooth_delta(
                    &mut scroll_position.y,
                    &mut delta.y,
                    max_offset.y,
                    contain,
                    source,
                    smooth_opt.as_deref_mut(),
                    false,
                    has_overscroll,
                );
            }
        } else {
            apply_normal_or_smooth_delta(
                &mut scroll_position.y,
                &mut delta.y,
                max_offset.y,
                contain,
                source,
                smooth_opt.as_deref_mut(),
                false,
                has_overscroll,
            );
        }
    }

    // overscroll 触摸拖动：中断回弹 spring（手指接管位置）
    if source == ScrollSource::Touch && has_overscroll {
        if let Some(entry) = bounce_state.entries.get_mut(&scroll_entity) {
            entry.spring_x = None;
            entry.spring_y = None;
        }
    }

    if *delta == Vec2::ZERO {
        scroll.propagate(false);
    }
}
