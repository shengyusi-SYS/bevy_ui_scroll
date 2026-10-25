//! 滚动 delta 应用：平滑滚动、overscroll 阻尼、snap delta。

use bevy::{
    ecs::{
        component::Component,
        message::MessageWriter,
        system::{Query, Res},
    },
    math::Vec2,
    time::Time,
    ui::{ComputedNode, Node, OverflowAxis, ScrollPosition},
    window::RequestRedraw,
};

use super::super::overscroll::{NORMAL_MAX_OVERSCROLL, NORMAL_OVERSCROLL_RESISTANCE};
use super::super::snap::{MAX_OVERSCROLL, ScrollSnapState, TOUCH_OVERSCROLL_RESISTANCE};
use super::ScrollSource;

// ── SmoothScroll ──

/// 标记容器使用保留重定向速度的行滚轮平滑滚动。
#[derive(Component)]
#[require(SmoothScrollState, crate::EnhancedScroll)]
pub struct SmoothScroll;

/// §6: 滚轮平滑滚动动画内部状态。
#[derive(Component, Default)]
pub(crate) struct SmoothScrollState {
    target: Vec2,
    // Keep the local trajectory independent of rounded absolute UI coordinates.
    remaining: Vec2,
    velocity: Vec2,
}

// One default response, close to the previous 250ms settle time. Retargeting
// changes only the destination; velocity survives until real input takes over.
const WHEEL_RESPONSE: f32 = 32.0;
const WHEEL_POSITION_EPSILON: f32 = 0.1;
const WHEEL_VELOCITY_EPSILON: f32 = 5.0;

/// 普通滚动或平滑滚动
pub(super) fn apply_normal_or_smooth_delta(
    pos: &mut f32,
    delta: &mut f32,
    max: f32,
    contain: bool,
    source: ScrollSource,
    smooth_opt: Option<&mut SmoothScrollState>,
    is_x: bool,
    overscroll: bool,
) {
    if source == ScrollSource::Wheel {
        if let Some(smooth) = smooth_opt {
            let axis = usize::from(!is_x);
            let active = smooth.remaining[axis] != 0.0 || smooth.velocity[axis] != 0.0;
            let previous_target = if active { smooth.target[axis] } else { *pos };
            let requested = previous_target.clamp(0.0, max.max(0.0));
            let consumed = (requested + *delta).clamp(0.0, max.max(0.0)) - requested;
            if consumed != 0.0 {
                let target = requested + consumed;
                smooth.remaining[axis] = if active {
                    smooth.remaining[axis] + (target - previous_target)
                } else {
                    target - *pos
                };
                smooth.target[axis] = target;
            }
            *delta = if contain || consumed != 0.0 { 0.0 } else { *delta };
            return;
        }
    }
    if source == ScrollSource::Touch && overscroll {
        apply_touch_overscroll_delta(pos, delta, max, contain);
    } else {
        apply_normal_delta(pos, delta, max, contain);
    }
}

/// 普通滚动：边界 clamp
fn apply_normal_delta(pos: &mut f32, delta: &mut f32, max: f32, contain: bool) {
    let max = max.max(0.0);
    let at_limit = if *delta > 0.0 { *pos >= max } else { *pos <= 0.0 };
    if !at_limit {
        *pos = (*pos + *delta).clamp(0.0, max);
        *delta = 0.0;
    } else if contain {
        *delta = 0.0;
    }
}

/// 触摸 overscroll 带阻尼的 delta 处理（普通容器版）
fn apply_touch_overscroll_delta(pos: &mut f32, delta: &mut f32, max: f32, contain: bool) {
    let max = max.max(0.0);
    let d = *delta;
    let new_val = *pos + d;

    if new_val < 0.0 {
        if d < 0.0 {
            let over = (-new_val).min(NORMAL_MAX_OVERSCROLL);
            let resistance = NORMAL_OVERSCROLL_RESISTANCE / (1.0 + over / NORMAL_MAX_OVERSCROLL);
            if *pos >= 0.0 {
                let delta_beyond = d + *pos;
                *pos = delta_beyond * resistance;
            } else {
                *pos += d * resistance;
            }
            *pos = (*pos).max(-NORMAL_MAX_OVERSCROLL);
        } else {
            *pos = new_val;
        }
    } else if new_val > max {
        if d > 0.0 {
            let over = (new_val - max).min(NORMAL_MAX_OVERSCROLL);
            let resistance = NORMAL_OVERSCROLL_RESISTANCE / (1.0 + over / NORMAL_MAX_OVERSCROLL);
            if *pos <= max {
                let delta_beyond = d - (max - *pos);
                *pos = max + delta_beyond * resistance;
            } else {
                *pos += d * resistance;
            }
            *pos = (*pos).min(max + NORMAL_MAX_OVERSCROLL);
        } else {
            *pos = new_val;
        }
    } else {
        *pos = new_val;
    }

    if contain
        || !(*pos <= 0.0 && d < 0.0 && *pos <= -NORMAL_MAX_OVERSCROLL + 0.1)
            && !(*pos >= max && d > 0.0 && *pos >= max + NORMAL_MAX_OVERSCROLL - 0.1)
    {
        *delta = 0.0;
    }
}

/// ScrollSnap 轴的 delta 处理
pub(crate) fn apply_snap_delta(
    pos: &mut f32,
    delta: f32,
    max: f32,
    source: ScrollSource,
    state_opt: Option<&mut ScrollSnapState>,
) {
    let Some(state) = state_opt else {
        *pos += delta;
        return;
    };

    state.suspended = false;
    if state.animating && state.touching {
        return;
    }

    if !state.touching {
        if source == ScrollSource::Touch {
            return;
        }
        state.wheel_delta += delta;
        *pos += delta;
        return;
    }

    let new_val = *pos + delta;

    if new_val < 0.0 {
        if delta < 0.0 {
            let over = (-new_val).min(MAX_OVERSCROLL);
            let resistance = TOUCH_OVERSCROLL_RESISTANCE / (1.0 + over / MAX_OVERSCROLL);
            if *pos >= 0.0 {
                let delta_beyond = delta + *pos;
                *pos = delta_beyond * resistance;
            } else {
                *pos += delta * resistance;
            }
            *pos = (*pos).max(-MAX_OVERSCROLL);
        } else {
            *pos = new_val;
        }
    } else if new_val > max {
        if delta > 0.0 {
            let over = (new_val - max).min(MAX_OVERSCROLL);
            let resistance = TOUCH_OVERSCROLL_RESISTANCE / (1.0 + over / MAX_OVERSCROLL);
            if *pos <= max {
                let delta_beyond = delta - (max - *pos);
                *pos = max + delta_beyond * resistance;
            } else {
                *pos += delta * resistance;
            }
            *pos = (*pos).min(max + MAX_OVERSCROLL);
        } else {
            *pos = new_val;
        }
    } else {
        *pos = new_val;
    }
}

// ── Smooth Scroll 系统 ──

/// §6: 滚轮平滑滚动动画系统。
pub(crate) fn smooth_scroll_system(
    mut query: Query<(&mut ScrollPosition, &Node, &ComputedNode, &mut SmoothScrollState)>,
    time: Res<Time>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    let mut has_active = false;
    for (mut position, node, computed, mut smooth) in query.iter_mut() {
        if smooth.remaining == Vec2::ZERO && smooth.velocity == Vec2::ZERO {
            continue;
        }
        has_active = true;
        let max = crate::max_scroll_offset(computed);
        for (axis, overflow) in [node.overflow.x, node.overflow.y].into_iter().enumerate() {
            if overflow != OverflowAxis::Scroll {
                smooth.remaining[axis] = 0.0;
                smooth.velocity[axis] = 0.0;
                continue;
            }
            if smooth.remaining[axis] == 0.0 && smooth.velocity[axis] == 0.0 {
                continue;
            }
            let target = smooth.target[axis].clamp(0.0, max[axis]);
            smooth.remaining[axis] += target - smooth.target[axis];
            smooth.target[axis] = target;
            let a = -smooth.remaining[axis];
            let b = smooth.velocity[axis] + WHEEL_RESPONSE * a;
            let (displacement, mut velocity) = crate::motion::spring_sample(a, b, WHEEL_RESPONSE, dt);
            let raw = target + displacement;
            let mut next = raw.clamp(0.0, max[axis]);
            let mut remaining = -displacement;
            if next != raw {
                velocity = 0.0;
                remaining = target - next;
            }
            if remaining.abs() <= WHEEL_POSITION_EPSILON && velocity.abs() <= WHEEL_VELOCITY_EPSILON {
                next = target;
                remaining = 0.0;
                velocity = 0.0;
            }
            if position.0[axis] != next {
                position.0[axis] = next;
            }
            smooth.remaining[axis] = remaining;
            smooth.velocity[axis] = velocity;
        }
    }
    if has_active {
        redraw.write(RequestRedraw);
    }
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    // ── apply_normal_delta ──

    #[test]
    fn apply_normal_delta_crossing_boundary_clamps_and_reverses_immediately() {
        // T0: crossing consumes this wheel event, while the next event can move back immediately.
        for (start, crossing, boundary, reverse, expected) in
            [(50.0, 1000.0, 500.0, -30.0, 470.0), (450.0, -1000.0, 0.0, 30.0, 30.0)]
        {
            for contain in [false, true] {
                let mut pos = start;
                let mut delta = crossing;
                apply_normal_delta(&mut pos, &mut delta, 500.0, contain);
                assert_eq!(pos, boundary);
                assert_eq!(delta, 0.0);

                delta = reverse;
                apply_normal_delta(&mut pos, &mut delta, 500.0, contain);
                assert_eq!(pos, expected);
                assert_eq!(delta, 0.0);
            }
        }
    }

    #[test]
    fn apply_normal_delta_within_bounds() {
        let mut pos = 50.0;
        let mut delta = 30.0;
        apply_normal_delta(&mut pos, &mut delta, 500.0, false);
        assert!((pos - 80.0).abs() < 0.01);
        assert!((delta).abs() < 0.01);
    }

    #[test]
    fn apply_normal_delta_at_top_boundary() {
        let mut pos = 0.0;
        let mut delta = -30.0;
        apply_normal_delta(&mut pos, &mut delta, 500.0, false);
        assert!((pos).abs() < 0.01);
        assert!((delta - (-30.0)).abs() < 0.01);
    }

    #[test]
    fn apply_normal_delta_at_bottom_boundary() {
        let mut pos = 500.0;
        let mut delta = 30.0;
        apply_normal_delta(&mut pos, &mut delta, 500.0, false);
        assert!((pos - 500.0).abs() < 0.01);
        assert!((delta - 30.0).abs() < 0.01);
    }

    #[test]
    fn apply_normal_delta_contain_mode() {
        let mut pos = 0.0;
        let mut delta = -30.0;
        apply_normal_delta(&mut pos, &mut delta, 500.0, true);
        assert!((pos).abs() < 0.01);
        assert!((delta).abs() < 0.01);
    }

    // ── §6 Smooth Scroll ──

    #[test]
    fn smooth_scroll_consumes_wheel_without_an_immediate_position_write() {
        let mut smooth = SmoothScrollState::default();
        let mut pos = 50.0;
        let mut delta = 60.0;
        apply_normal_or_smooth_delta(
            &mut pos,
            &mut delta,
            500.0,
            false,
            ScrollSource::Wheel,
            Some(&mut smooth),
            false,
            false,
        );
        assert_eq!(delta, 0.0);
        assert_eq!(pos, 50.0);
    }

    #[test]
    fn smooth_scroll_touch_bypasses() {
        let mut smooth = SmoothScrollState::default();
        let mut pos = 50.0;
        let mut delta = 60.0;
        apply_normal_or_smooth_delta(
            &mut pos,
            &mut delta,
            500.0,
            false,
            ScrollSource::Touch,
            Some(&mut smooth),
            false,
            false,
        );
        assert_eq!(pos, 110.0);
        assert_eq!(delta, 0.0);
    }

    #[test]
    fn smooth_scroll_no_state_direct_apply() {
        let mut pos = 50.0;
        let mut delta = 60.0;
        apply_normal_or_smooth_delta(
            &mut pos,
            &mut delta,
            500.0,
            false,
            ScrollSource::Wheel,
            None,
            false,
            false,
        );
        assert_eq!(pos, 110.0);
        assert_eq!(delta, 0.0);
    }
}
