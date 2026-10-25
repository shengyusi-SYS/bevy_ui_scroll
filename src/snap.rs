//! Scroll-snap 实现。
//!
//! 对齐 Chrome scroll-snap：
//! - 指数衰减惯性（Chrome OverScroller 风格）
//! - 时间窗口速度采样（Chrome VelocityTracker 风格）
//! - 方向偏好 snap 选择（松手时预判，惯性结束时使用）
//! - 考虑 gap 的 snap 点收集
//! - 触摸拖动 over-scroll 阻尼（在 delta 应用阶段完成）
//! - 惯性 over-scroll 橡皮筋（阻尼 spring 回弹，目标为最近 snap 点）
//! - Snap 动画 velocity matching（critically damped spring）
//! - 鼠标滚轮按 snap 点跳转（支持快速连续翻页）

mod geometry;
use geometry::*;

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        event::EntityEvent,
        hierarchy::ChildOf,
        message::{MessageReader, MessageWriter},
        observer::On,
        system::{Query, Res},
    },
    input::touch::{TouchInput, TouchPhase},
    prelude::{Children, DetectChangesMut},
    time::Time,
    ui::{ComputedNode, ScrollPosition, UiGlobalTransform},
    window::RequestRedraw,
};

// ── 公开类型 ──

/// Scroll-snap 轴向
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScrollSnapAxis {
    X,
    Y,
}

/// 声明容器为 scroll-snap 容器（mandatory 模式）。
#[derive(Component)]
#[require(ScrollSnapState, ScrollSnapStatus, crate::EnhancedScroll)]
pub struct ScrollSnap {
    pub axis: ScrollSnapAxis,
}

/// 声明子元素为一个 snap 对齐点（对齐方式 = start）。
#[derive(Component)]
pub struct ScrollSnapPoint;

/// Select a snap point by stable entity or depth-first Children declaration order.
#[derive(Clone, Copy, Debug)]
pub enum SnapTarget {
    Entity(Entity),
    Index(usize),
}

/// Programmatic snap navigation; invalid/non-descendant targets are ignored.
#[derive(EntityEvent)]
pub struct ScrollToSnap {
    pub entity: Entity,
    pub target: SnapTarget,
    pub instant: bool,
}

/// Observed after native layout. A chosen point can precede physical settlement.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollSnapStatus {
    pub selected: Option<Entity>,
    pub settled: Option<Entity>,
}

// ── 常量 ──

use super::event::{DirectionLock, TouchScrollState};
use super::fling::FLING_DECAY;
use super::gesture_capture::TouchGestureCaptureState;
use super::velocity::SingleAxisVelocitySampler;

/// 惯性结束速度阈值 (px/s)
const MIN_FLING_VELOCITY: f32 = 50.0;
/// Snap spring 自然频率 ω (rad/s)（≈300ms 到位）
const SNAP_SPRING_OMEGA: f32 = 20.0;
/// Snap 动画到位阈值 (px)
const SNAP_THRESHOLD: f32 = 0.5;
/// Over-scroll 最大弹性距离 (px)
pub(super) const MAX_OVERSCROLL: f32 = 150.0;
/// 触摸拖动 over-scroll 阻尼系数（0~1，越小阻力越大）
pub(super) const TOUCH_OVERSCROLL_RESISTANCE: f32 = 0.3;
/// 鼠标滚轮连续 snap 的 debounce 时间 (秒)
const WHEEL_IDLE_TIMEOUT: f32 = 0.1;

// ── ScrollSnapState ──

/// Scroll-snap 内部惯性和动画状态。
#[derive(Component)]
pub(crate) struct ScrollSnapState {
    pub(crate) suspended: bool,
    /// 当前惯性速度 (px/s)
    velocity: f32,
    /// 上一帧的 scroll 位置
    prev_pos: f32,
    /// 是否正在触摸拖动中
    pub(super) touching: bool,
    /// 是否正在 spring 动画中（阻止 scroll handler 干扰）
    pub(super) animating: bool,
    /// Snap / over-scroll 回弹 spring 动画
    snap_anim: Option<SpringAnim>,
    /// 速度采样器
    sampler: SingleAxisVelocitySampler,
    /// 上次滚轮事件的时间戳（用于 idle 检测）
    wheel_last_time: f32,
    /// 是否正在 wheel 自由滚动中（Chrome 模型：wheel 期间不 snap）
    wheel_scrolling: bool,
    /// D5: 绑定的触摸 ID（只响应起始于容器区域内的触摸）
    active_touch: Option<u64>,
    /// §9: 本帧累积的滚轮 delta（由 apply_snap_delta 记录，scroll_snap_system 消费）
    pub(super) wheel_delta: f32,
}

impl Default for ScrollSnapState {
    fn default() -> Self {
        Self {
            suspended: false,
            velocity: 0.0,
            prev_pos: 0.0,
            touching: false,
            animating: false,
            snap_anim: None,
            sampler: SingleAxisVelocitySampler::default(),
            wheel_last_time: 0.0,
            wheel_scrolling: false,
            active_touch: None,
            wheel_delta: 0.0,
        }
    }
}

/// 跟踪触摸输入：速度采样 + 松手时预判 snap 目标。
pub(super) fn scroll_snap_touch_tracker(
    mut touch_events: MessageReader<TouchInput>,
    mut snap_q: Query<(
        Entity,
        &mut ScrollPosition,
        &ScrollSnap,
        &mut ScrollSnapState,
        &ComputedNode,
        &UiGlobalTransform,
    )>,
    children_q: Query<&Children>,
    snap_child_q: Query<(&UiGlobalTransform, &ComputedNode), bevy::ecs::query::With<ScrollSnapPoint>>,
    snap_container_q: Query<(), bevy::ecs::query::With<ScrollSnap>>,
    parent_q: Query<&ChildOf>,
    time: Res<Time>,
    touch_scroll_state: Res<TouchScrollState>,
    captures: Res<TouchGestureCaptureState>,
) {
    let base_time = time.elapsed_secs();
    let mut event_idx = 0u32;

    for touch in touch_events.read() {
        let now = base_time + event_idx as f32 * 0.0001;
        event_idx += 1;

        // Started 深度排他 pre-pass：只让最内层 snap 容器响应触摸开始
        let deepest_started = if touch.phase == TouchPhase::Started && !touch_is_captured(&captures, touch.id) {
            let mut best: Option<(Entity, u32)> = None;
            for (entity, _, _, _, computed, global_transform) in snap_q.iter() {
                if point_in_node(touch.position, computed, global_transform) {
                    let depth = super::event::entity_depth(entity, &parent_q);
                    if best.map_or(true, |(_, d)| depth > d) {
                        best = Some((entity, depth));
                    }
                }
            }
            best.map(|(e, _)| e)
        } else {
            None
        };

        for (entity, mut scroll, snap_cfg, mut state, computed, global_transform) in snap_q.iter_mut() {
            let current = get_scroll(&scroll, snap_cfg.axis);

            match touch.phase {
                TouchPhase::Started => {
                    // pre-pass 已选出最深容器，仅激活 deepest
                    if deepest_started != Some(entity) {
                        continue;
                    }

                    // 中断一切动画和惯性
                    state.suspended = false;
                    state.touching = true;
                    state.active_touch = Some(touch.id);
                    state.snap_anim = None;
                    state.animating = false;
                    state.velocity = 0.0;
                    state.sampler.reset();
                    state.wheel_scrolling = false;

                    // #7: 如果当前在越界位置，clamp 回有效范围
                    let max_scroll = compute_max_scroll(snap_cfg.axis, &computed);
                    let clamped = current.clamp(0.0, max_scroll);
                    if (current - clamped).abs() > 0.1 {
                        set_scroll(&mut scroll, snap_cfg.axis, clamped);
                    }

                    state.prev_pos = get_scroll(&scroll, snap_cfg.axis);
                    let touch_coord = get_touch_axis(touch.position, snap_cfg.axis);
                    state.sampler.add_sample(now, touch_coord);
                }
                TouchPhase::Moved => {
                    if !state.touching || state.active_touch != Some(touch.id) {
                        continue;
                    }
                    if touch_is_captured(&captures, touch.id) {
                        clear_active_touch_tracking(&mut state, current);
                        continue;
                    }
                    // scroll session 已确定 target 且不是当前容器 → 停止采样，让出给内层
                    if let Some(scroll_target) = touch_scroll_state.scroll_target(touch.id) {
                        if scroll_target != entity {
                            clear_active_touch_tracking(&mut state, current);
                            continue;
                        }
                    }
                    // over-scroll 阻尼已在 on_scroll_handler 中处理
                    // 这里只做速度采样
                    let touch_coord = get_touch_axis(touch.position, snap_cfg.axis);
                    state.sampler.add_sample(now, touch_coord);
                    state.prev_pos = get_scroll(&scroll, snap_cfg.axis);
                }
                TouchPhase::Ended => {
                    if !state.touching || state.active_touch != Some(touch.id) {
                        continue;
                    }
                    if touch_is_captured(&captures, touch.id) {
                        clear_active_touch_tracking(&mut state, current);
                        continue;
                    }
                    state.touching = false;
                    state.active_touch = None;

                    // Scroll Session: tap（未超过 TOUCH_SLOP）或方向锁定与 snap 轴不匹配 → 跳过 snap 动画
                    if let Some(lock) = touch_scroll_state.direction_lock(touch.id) {
                        let skip = match lock {
                            DirectionLock::None => true,
                            DirectionLock::Horizontal => snap_cfg.axis == ScrollSnapAxis::Y,
                            DirectionLock::Vertical => snap_cfg.axis == ScrollSnapAxis::X,
                            DirectionLock::Free => false,
                        };
                        if skip {
                            state.velocity = 0.0;
                            continue;
                        }
                    }

                    // Scroll Session: 如果滚动已被内层容器消费，外层 snap 不触发
                    // scroll_target 是 Scroll Session 选择的唯一滚动目标（最内层可滚动容器）
                    // 只有 scroll_target == 当前 snap entity 才说明本容器是真正的滚动目标
                    if let Some(scroll_target) = touch_scroll_state.scroll_target(touch.id) {
                        if scroll_target != entity {
                            state.velocity = 0.0;
                            continue;
                        }
                    }

                    // Chrome VelocityTracker: UP 事件不添加样本，只用已有数据计算
                    // 速度取反：屏幕触摸向右 → scroll 向左（负方向）
                    let fling_velocity = -state.sampler.compute_velocity(now);
                    state.prev_pos = current;

                    let max_scroll = compute_max_scroll(snap_cfg.axis, &computed);

                    // #1: 如果当前在越界位置，直接启动回弹 spring，不走惯性
                    if current < 0.0 || current > max_scroll {
                        let points = collect_snap_points(
                            snap_cfg.axis,
                            entity,
                            global_transform,
                            computed,
                            &children_q,
                            &snap_child_q,
                            &snap_container_q,
                        );
                        let boundary = if current < 0.0 { 0.0 } else { max_scroll };
                        let target = nearest_snap_point(&points, boundary).unwrap_or(boundary);
                        state.snap_anim = Some(SpringAnim::new(current, target, fling_velocity, SNAP_SPRING_OMEGA));
                        state.velocity = 0.0;
                        continue;
                    }

                    // 预判惯性终点（指数衰减：total_distance = v / decay）
                    let decel_dist = if fling_velocity.abs() > MIN_FLING_VELOCITY {
                        fling_velocity / FLING_DECAY
                    } else {
                        0.0
                    };
                    let predicted_end = current + decel_dist;
                    let direction = fling_velocity.signum();

                    let points = collect_snap_points(
                        snap_cfg.axis,
                        entity,
                        global_transform,
                        computed,
                        &children_q,
                        &snap_child_q,
                        &snap_container_q,
                    );
                    let snap_target = directional_snap_point(&points, predicted_end, current, direction);

                    // Chrome 风格：直接启动 snap spring，不走惯性阶段
                    // spring 带着完整 fling_velocity 直接向 target 运动，
                    // 避免先减速再加速的问题
                    if let Some(target) = snap_target {
                        if (current - target).abs() > SNAP_THRESHOLD {
                            state.snap_anim = Some(SpringAnim::new(current, target, fling_velocity, SNAP_SPRING_OMEGA));
                        } else {
                            set_scroll(&mut scroll, snap_cfg.axis, target);
                            state.prev_pos = target;
                        }
                        state.velocity = 0.0;
                    } else {
                        // 无 snap 点降级：走惯性
                        state.velocity = fling_velocity;
                    }
                }
                TouchPhase::Canceled => {
                    // D18: Cancel 不产生 fling，直接 snap 到最近点（零速度 spring）
                    if !state.touching || state.active_touch != Some(touch.id) {
                        continue;
                    }
                    if touch_is_captured(&captures, touch.id) {
                        clear_active_touch_tracking(&mut state, current);
                        continue;
                    }
                    state.touching = false;
                    state.active_touch = None;
                    state.velocity = 0.0;
                    state.prev_pos = current;

                    let max_scroll = compute_max_scroll(snap_cfg.axis, &computed);
                    let clamped = current.clamp(0.0, max_scroll);
                    let points = collect_snap_points(
                        snap_cfg.axis,
                        entity,
                        global_transform,
                        computed,
                        &children_q,
                        &snap_child_q,
                        &snap_container_q,
                    );
                    let target = nearest_snap_point(&points, clamped).unwrap_or(clamped);
                    if (current - target).abs() > SNAP_THRESHOLD {
                        state.snap_anim = Some(SpringAnim::new(current, target, 0.0, SNAP_SPRING_OMEGA));
                    } else {
                        set_scroll(&mut scroll, snap_cfg.axis, target);
                        state.prev_pos = target;
                    }
                }
            }
        }
    }
}

fn touch_is_captured(captures: &TouchGestureCaptureState, touch_id: u64) -> bool {
    captures.owner(touch_id).is_some()
}

fn clear_active_touch_tracking(state: &mut ScrollSnapState, current: f32) {
    state.touching = false;
    state.active_touch = None;
    state.velocity = 0.0;
    state.prev_pos = current;
    state.sampler.reset();
}

/// 惯性滚动 + snap 吸附 + over-scroll 回弹。
pub(super) fn scroll_snap_system(
    mut snap_q: Query<(
        Entity,
        &mut ScrollPosition,
        &ScrollSnap,
        &mut ScrollSnapState,
        &ComputedNode,
        &UiGlobalTransform,
    )>,
    children_q: Query<&Children>,
    snap_child_q: Query<(&UiGlobalTransform, &ComputedNode), bevy::ecs::query::With<ScrollSnapPoint>>,
    snap_container_q: Query<(), bevy::ecs::query::With<ScrollSnap>>,
    time: Res<Time>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (entity, mut scroll, snap_cfg, mut state, computed, global_transform) in snap_q.iter_mut() {
        let current = get_scroll(&scroll, snap_cfg.axis);
        let max_scroll = compute_max_scroll(snap_cfg.axis, computed);

        if state.suspended {
            continue;
        }

        // ── 触摸中：不做任何动画 ──
        if state.touching {
            state.prev_pos = current;
            continue;
        }

        // ── Chrome 模型：wheel 自由滚动 → idle 检测 → snap ──
        let now = time.elapsed_secs();
        let has_new_wheel = state.wheel_delta.abs() > 0.001;
        if has_new_wheel {
            state.wheel_last_time = now;
            state.wheel_scrolling = true;
            state.wheel_delta = 0.0;
            // wheel 到来 → 取消正在进行的 snap 动画，回到自由滚动
            if state.snap_anim.is_some() {
                state.snap_anim = None;
                state.animating = false;
            }
        }
        // wheel idle 检测：超过 timeout 且不在动画中 → 触发 snap
        if state.wheel_scrolling && !has_new_wheel && (now - state.wheel_last_time) >= WHEEL_IDLE_TIMEOUT {
            state.wheel_scrolling = false;
            let points = collect_snap_points(
                snap_cfg.axis,
                entity,
                global_transform,
                computed,
                &children_q,
                &snap_child_q,
                &snap_container_q,
            );
            if let Some(target) = nearest_snap_point(&points, current) {
                if (current - target).abs() > SNAP_THRESHOLD {
                    state.snap_anim = Some(SpringAnim::new(current, target, 0.0, SNAP_SPRING_OMEGA));
                } else {
                    set_scroll(&mut scroll, snap_cfg.axis, target);
                }
            }
            state.prev_pos = get_scroll(&scroll, snap_cfg.axis);
            continue;
        }
        // wheel 自由滚动中：不做任何动画/snap
        if state.wheel_scrolling {
            state.prev_pos = current;
            continue;
        }

        // ── Spring 动画进行中（snap 吸附 或 over-scroll 回弹共用）──
        if state.snap_anim.is_some() {
            state.animating = true;
            let anim = state.snap_anim.as_mut().unwrap();
            let (new_val, _vel, done) = anim.update(dt);
            let target = anim.target;
            if done {
                set_scroll(&mut scroll, snap_cfg.axis, target);
                state.snap_anim = None;
                state.animating = false;
            } else {
                set_scroll(&mut scroll, snap_cfg.axis, new_val);
            }
            state.prev_pos = get_scroll(&scroll, snap_cfg.axis);
            state.velocity = 0.0;
            continue;
        }

        state.animating = false;

        // ── 惯性滑行（指数衰减 — 与 event.rs fling 统一）──
        if state.velocity.abs() > MIN_FLING_VELOCITY {
            // 先用当前速度算位移
            let new_val = current + state.velocity * dt;

            // 指数衰减：v(t+dt) = v(t) * e^(-decay * dt)
            state.velocity *= (-FLING_DECAY * dt).exp();

            // 边界处理：进入 over-scroll → 立即启动回弹 spring
            if new_val < 0.0 {
                set_scroll(&mut scroll, snap_cfg.axis, new_val);
                let points = collect_snap_points(
                    snap_cfg.axis,
                    entity,
                    global_transform,
                    computed,
                    &children_q,
                    &snap_child_q,
                    &snap_container_q,
                );
                let target = nearest_snap_point(&points, 0.0).unwrap_or(0.0);
                state.snap_anim = Some(SpringAnim::new(new_val, target, state.velocity, SNAP_SPRING_OMEGA));
                state.velocity = 0.0;
            } else if new_val > max_scroll {
                set_scroll(&mut scroll, snap_cfg.axis, new_val);
                let points = collect_snap_points(
                    snap_cfg.axis,
                    entity,
                    global_transform,
                    computed,
                    &children_q,
                    &snap_child_q,
                    &snap_container_q,
                );
                let target = nearest_snap_point(&points, max_scroll).unwrap_or(max_scroll);
                state.snap_anim = Some(SpringAnim::new(new_val, target, state.velocity, SNAP_SPRING_OMEGA));
                state.velocity = 0.0;
            } else {
                set_scroll(&mut scroll, snap_cfg.axis, new_val);
            }
            state.prev_pos = get_scroll(&scroll, snap_cfg.axis);

            // 速度降到阈值 → snap 到最近 snap 点
            if state.velocity.abs() <= MIN_FLING_VELOCITY && state.snap_anim.is_none() {
                let residual_v = state.velocity;
                state.velocity = 0.0;
                let pos = get_scroll(&scroll, snap_cfg.axis);

                let points = collect_snap_points(
                    snap_cfg.axis,
                    entity,
                    global_transform,
                    computed,
                    &children_q,
                    &snap_child_q,
                    &snap_container_q,
                );
                let target = nearest_snap_point(&points, pos);

                if let Some(target) = target {
                    if (pos - target).abs() > SNAP_THRESHOLD {
                        state.snap_anim = Some(SpringAnim::new(pos, target, residual_v, SNAP_SPRING_OMEGA));
                    } else {
                        set_scroll(&mut scroll, snap_cfg.axis, target);
                    }
                }
                state.prev_pos = get_scroll(&scroll, snap_cfg.axis);
            }
            continue;
        }

        // ── D17: Layout Shift 后 Re-snap ──
        // 空闲状态（无惯性、无动画、无触摸、无 wheel 滚动）时，
        // 检查当前位置是否偏离最近 snap 点，如果偏离则启动 spring 对齐。
        let points = collect_snap_points(
            snap_cfg.axis,
            entity,
            global_transform,
            computed,
            &children_q,
            &snap_child_q,
            &snap_container_q,
        );
        if let Some(target) = nearest_snap_point(&points, current) {
            if (current - target).abs() > SNAP_THRESHOLD {
                bevy::log::trace!(
                    "[D17 Re-snap] current={}, target={}, points={:?}",
                    current,
                    target,
                    points
                );
                state.snap_anim = Some(SpringAnim::new(current, target, 0.0, SNAP_SPRING_OMEGA));
            }
        }
    }

    // 有活跃惯性/snap 动画 → 请求下一帧重绘
    for (_, _, _, state, _, _) in snap_q.iter() {
        if state.velocity.abs() > MIN_FLING_VELOCITY || state.snap_anim.is_some() || state.wheel_scrolling {
            redraw.write(RequestRedraw);
            break;
        }
    }
}

#[derive(bevy::prelude::Component, Clone, Copy)]
pub(crate) struct PendingSnapRequest {
    target: SnapTarget,
    instant: bool,
}

pub(super) fn on_scroll_to_snap(
    event: On<ScrollToSnap>,
    containers: Query<(), bevy::prelude::With<ScrollSnap>>,
    mut commands: bevy::prelude::Commands,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    if containers.contains(event.entity) {
        commands.entity(event.entity).insert(PendingSnapRequest {
            target: event.target,
            instant: event.instant,
        });
        redraw.write(RequestRedraw);
    }
}

// Resolve after native layout so responsive Node changes cannot reuse stale snap coordinates.
pub(crate) fn resolve_snap_requests(
    containers: Query<(
        Entity,
        &ScrollSnap,
        &ComputedNode,
        &UiGlobalTransform,
        &PendingSnapRequest,
    )>,
    children: Query<&Children>,
    points: Query<(&UiGlobalTransform, &ComputedNode), bevy::prelude::With<ScrollSnapPoint>>,
    nested: Query<(), bevy::prelude::With<ScrollSnap>>,
    mut commands: bevy::prelude::Commands,
) {
    for (entity, config, computed, transform, request) in &containers {
        let (config, computed, transform) = (config, computed, transform);
        if computed.size().min_element() <= 0.0 {
            continue;
        }
        let targets = collect_snap_targets(config.axis, entity, transform, computed, &children, &points, &nested);
        let target = match request.target {
            SnapTarget::Entity(entity) => targets.iter().find(|(point, _)| *point == entity),
            SnapTarget::Index(index) => targets.get(index),
        };
        let Some(&(point, offset)) = target else {
            commands.entity(entity).remove::<PendingSnapRequest>();
            continue;
        };
        let (entity, instant, axis) = (entity, request.instant, config.axis);
        let offset = offset.clamp(0.0, compute_max_scroll(axis, computed));
        commands.queue(move |world: &mut bevy::prelude::World| {
            crate::navigation::stop_motion(world, entity);
            let Some(position) = world.get::<ScrollPosition>(entity) else {
                return;
            };
            let current = get_scroll(position, axis);
            let animate = !instant && (current - offset).abs() > SNAP_THRESHOLD;
            if !animate && let Some(mut position) = world.get_mut::<ScrollPosition>(entity) {
                set_scroll(&mut position, axis, offset);
            }
            if let Some(mut state) = world.get_mut::<ScrollSnapState>(entity) {
                *state = ScrollSnapState::default();
                state.prev_pos = current;
                if animate {
                    state.snap_anim = Some(SpringAnim::new(current, offset, 0.0, SNAP_SPRING_OMEGA));
                    state.animating = true;
                }
            }
            if let Some(mut status) = world.get_mut::<ScrollSnapStatus>(entity) {
                status.set_if_neq(ScrollSnapStatus {
                    selected: Some(point),
                    settled: None,
                });
            }
            world.write_message(RequestRedraw);
        });
    }
}

pub(crate) fn observe_snap_status(
    mut containers: Query<(
        Entity,
        &ScrollSnap,
        &ScrollSnapState,
        &ScrollPosition,
        &ComputedNode,
        &UiGlobalTransform,
        &mut ScrollSnapStatus,
    )>,
    children: Query<&Children>,
    points: Query<(&UiGlobalTransform, &ComputedNode), bevy::prelude::With<ScrollSnapPoint>>,
    nested: Query<(), bevy::prelude::With<ScrollSnap>>,
) {
    use bevy::prelude::DetectChangesMut;
    for (entity, config, state, position, computed, transform, mut status) in &mut containers {
        let targets = collect_snap_targets(config.axis, entity, transform, computed, &children, &points, &nested);
        let actual = match config.axis {
            ScrollSnapAxis::X => computed.scroll_position.x,
            ScrollSnapAxis::Y => computed.scroll_position.y,
        } * computed.inverse_scale_factor();
        let idle = !state.touching
            && !state.wheel_scrolling
            && state.velocity.abs() <= MIN_FLING_VELOCITY
            && state.snap_anim.is_none();
        let choose = |value: f32| {
            let matches = |point: &&(Entity, f32)| (point.1 - value).abs() <= SNAP_THRESHOLD;
            // Several short tail items can share the clamped native limit.
            targets
                .iter()
                .filter(matches)
                .find(|(point, _)| Some(*point) == status.selected)
                .or_else(|| targets.iter().find(matches))
                .map(|(point, _)| *point)
        };
        let actual_point = if idle { choose(actual) } else { None };
        let requested = if idle && !state.suspended {
            choose(get_scroll(position, config.axis))
        } else {
            None
        };
        let selected = state
            .snap_anim
            .as_ref()
            .and_then(|anim| choose(anim.target))
            .or(requested)
            .or(actual_point);
        let settled = actual_point.filter(|point| Some(*point) == selected);
        status.set_if_neq(ScrollSnapStatus { selected, settled });
    }
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        input::{
            ButtonInput,
            keyboard::KeyCode,
            mouse::MouseWheel,
            touch::{TouchInput, TouchPhase},
        },
        math::{Affine2, Vec2},
        picking::{backend::HitData, hover::HoverMap, pointer::PointerId},
        prelude::{Entity, IntoScheduleConfigs, Messages, Node, Update},
        time::Time,
        ui::{ComputedNode, Overflow, ScrollPosition, UiGlobalTransform},
        window::RequestRedraw,
    };

    use crate::{
        ScrollGestureCapture,
        event::{TouchScrollState, send_scroll_events},
        gesture_capture::TouchGestureCaptureState,
    };

    use super::*;

    // ══════════════════════════════════════════════
    // SingleAxisVelocitySampler（对应 Chrome scroll_velocity_tracker_unittest.cc）
    // ══════════════════════════════════════════════

    /// 零样本返回零速度
    #[test]
    fn velocity_sampler_no_samples() {
        let sampler = SingleAxisVelocitySampler::default();
        assert_eq!(sampler.compute_velocity(1.0), 0.0);
    }

    /// 单样本返回零速度（无法计算差分）
    #[test]
    fn velocity_sampler_single_sample() {
        let mut sampler = SingleAxisVelocitySampler::default();
        sampler.add_sample(1.0, 100.0);
        assert_eq!(sampler.compute_velocity(1.0), 0.0);
    }

    /// 线性运动产生正确速度
    #[test]
    fn velocity_sampler_linear() {
        let mut sampler = SingleAxisVelocitySampler::default();
        // 200 px/s 线性运动
        sampler.add_sample(1.00, 0.0);
        sampler.add_sample(1.02, 4.0);
        sampler.add_sample(1.04, 8.0);
        sampler.add_sample(1.06, 12.0);
        sampler.add_sample(1.08, 16.0);
        let v = sampler.compute_velocity(1.08);
        assert!((v - 200.0).abs() < 1.0, "velocity: {}", v);
    }

    /// 超过窗口期的旧样本被丢弃
    #[test]
    fn velocity_sampler_window_cutoff() {
        let mut sampler = SingleAxisVelocitySampler::default();
        // 旧样本（高速）
        sampler.add_sample(0.0, 0.0);
        sampler.add_sample(0.05, 500.0); // 10000 px/s
        // 窗口期内新样本（低速 100 px/s）
        sampler.add_sample(0.90, 200.0);
        sampler.add_sample(0.95, 205.0);
        sampler.add_sample(0.99, 209.0);
        let v = sampler.compute_velocity(0.99);
        // 窗口 cutoff=0.89 → (209-200)/(0.99-0.90) = 100 px/s
        assert!((v - 100.0).abs() < 1.0, "velocity: {}", v);
    }

    /// 同时间戳不导致除零
    #[test]
    fn velocity_sampler_same_timestamp() {
        let mut sampler = SingleAxisVelocitySampler::default();
        sampler.add_sample(1.0, 0.0);
        sampler.add_sample(1.0, 100.0);
        assert_eq!(sampler.compute_velocity(1.0), 0.0);
    }

    /// reset 清除所有样本
    #[test]
    fn velocity_sampler_reset() {
        let mut sampler = SingleAxisVelocitySampler::default();
        sampler.add_sample(1.0, 0.0);
        sampler.add_sample(1.05, 50.0);
        sampler.reset();
        assert_eq!(sampler.compute_velocity(1.05), 0.0);
    }

    /// 缓冲区容量上限
    #[test]
    fn velocity_sampler_capacity() {
        let mut sampler = SingleAxisVelocitySampler::default();
        for i in 0..25 {
            sampler.add_sample(i as f32 * 0.01, i as f32 * 10.0);
        }
        // 容量由 velocity.rs 内部管理，只验证不 panic
    }

    /// 负方向速度
    #[test]
    fn velocity_sampler_negative() {
        let mut sampler = SingleAxisVelocitySampler::default();
        sampler.add_sample(1.00, 100.0);
        sampler.add_sample(1.05, 50.0);
        sampler.add_sample(1.09, 10.0);
        let v = sampler.compute_velocity(1.09);
        assert!((v - (-1000.0)).abs() < 1.0, "velocity: {}", v);
    }

    // ── apply_snap_delta 数值验证 ──

    use super::super::event::ScrollSource;

    /// 正常区域内直接应用 delta
    #[test]
    fn apply_snap_delta_normal_range() {
        let mut pos = 50.0;
        let mut state = ScrollSnapState::default();
        state.touching = true;
        super::super::event::apply_snap_delta(&mut pos, 30.0, 500.0, ScrollSource::Touch, Some(&mut state));
        assert!((pos - 80.0).abs() < 0.01);
    }

    /// 越界区域施加阻尼（远离边界方向 delta 减弱）
    #[test]
    fn apply_snap_delta_overscroll_damping() {
        let mut pos = 0.0;
        let mut state = ScrollSnapState::default();
        state.touching = true;
        super::super::event::apply_snap_delta(&mut pos, -100.0, 500.0, ScrollSource::Touch, Some(&mut state));
        // 阻尼后实际位移 < 100
        assert!(pos > -100.0, "pos: {} should be > -100 (damped)", pos);
        assert!(pos < 0.0, "pos: {} should be < 0 (did move)", pos);
        assert!(pos >= -MAX_OVERSCROLL);
    }

    /// 回拖方向 1:1 不施加阻尼
    #[test]
    fn apply_snap_delta_return_from_overscroll() {
        let mut pos = -50.0;
        let mut state = ScrollSnapState::default();
        state.touching = true;
        super::super::event::apply_snap_delta(&mut pos, 30.0, 500.0, ScrollSource::Touch, Some(&mut state));
        assert!((pos - (-20.0)).abs() < 0.01, "pos: {} should be -20 (1:1 return)", pos);
    }

    /// 触摸 + 动画中 → delta 被忽略
    #[test]
    fn apply_snap_delta_touching_and_animating() {
        let mut pos = 50.0;
        let mut state = ScrollSnapState::default();
        state.touching = true;
        state.animating = true;
        super::super::event::apply_snap_delta(&mut pos, 100.0, 500.0, ScrollSource::Touch, Some(&mut state));
        assert!((pos - 50.0).abs() < 0.01, "should ignore delta when touching+animating");
    }

    /// §2: 非触摸状态 + Wheel → 直接应用（scroll_snap_system 检测变化并 re-snap）
    #[test]
    fn apply_snap_delta_not_touching_wheel() {
        let mut pos = 50.0;
        let mut state = ScrollSnapState::default();
        state.touching = false;
        super::super::event::apply_snap_delta(&mut pos, 30.0, 500.0, ScrollSource::Wheel, Some(&mut state));
        assert!((pos - 80.0).abs() < 0.01);
    }

    /// §2: 非触摸状态 + Touch → 忽略（snap 容器的 fling 由 snap 系统驱动）
    #[test]
    fn apply_snap_delta_not_touching_touch_rejected() {
        let mut pos = 50.0;
        let mut state = ScrollSnapState::default();
        state.touching = false;
        super::super::event::apply_snap_delta(&mut pos, 30.0, 500.0, ScrollSource::Touch, Some(&mut state));
        assert!(
            (pos - 50.0).abs() < 0.01,
            "touch fling delta should be rejected for snap container"
        );
    }

    /// 无 snap 状态 → 直接应用
    #[test]
    fn apply_snap_delta_no_state() {
        let mut pos = 50.0;
        super::super::event::apply_snap_delta(&mut pos, 30.0, 500.0, ScrollSource::Wheel, None);
        assert!((pos - 80.0).abs() < 0.01);
    }

    #[test]
    fn captured_touch_does_not_activate_snap_container() {
        let mut app = snap_touch_tracker_test_app();
        let (container, _) = spawn_snap_container(&mut app, 40.0);
        let capture_owner = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<TouchGestureCaptureState>()
            .capture(7, capture_owner);

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 7, Vec2::new(10.0, 10.0)));
        app.update();

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert!(state.snap_anim.is_none());
    }

    #[test]
    fn slider_like_capture_blocks_parent_scroll_session() {
        let mut app = scroll_and_snap_regression_test_app();
        let (container, slider_like_child) = spawn_snap_container(&mut app, 40.0);

        {
            let mut hover_map = app.world_mut().resource_mut::<HoverMap>();
            insert_hover_hits(&mut hover_map, PointerId::Touch(21), &[slider_like_child]);
        }

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 21, Vec2::new(10.0, 10.0)));
        app.update();

        {
            let state = app.world().get::<ScrollSnapState>(container).unwrap();
            assert!(state.touching);
            assert_eq!(state.active_touch, Some(21));
        }

        app.world_mut()
            .resource_mut::<TouchGestureCaptureState>()
            .capture(21, slider_like_child);
        app.world_mut()
            .write_message(touch_input(TouchPhase::Moved, 21, Vec2::new(32.0, 10.0)));
        app.update();

        {
            let touch_state = app.world().resource::<TouchScrollState>();
            assert_eq!(touch_state.scroll_target(21), None);
        }

        {
            let state = app.world().get::<ScrollSnapState>(container).unwrap();
            assert!(!state.touching);
            assert_eq!(state.active_touch, None);
            assert!(state.snap_anim.is_none());
        }

        app.world_mut()
            .write_message(touch_input(TouchPhase::Ended, 21, Vec2::new(32.0, 10.0)));
        app.update();

        let touch_state = app.world().resource::<TouchScrollState>();
        assert!(matches!(
            touch_state.direction_lock(21),
            Some(DirectionLock::Horizontal)
        ));
        assert_eq!(touch_state.scroll_target(21), None);

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert!(state.snap_anim.is_none());
    }

    #[test]
    fn captured_touch_does_not_trigger_snap_on_touch_end() {
        let mut app = snap_touch_tracker_test_app();
        let (container, _) = spawn_snap_container(&mut app, 40.0);
        let capture_owner = app.world_mut().spawn_empty().id();

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 9, Vec2::new(10.0, 10.0)));
        app.update();

        {
            let state = app.world().get::<ScrollSnapState>(container).unwrap();
            assert!(state.touching);
            assert_eq!(state.active_touch, Some(9));
        }

        app.world_mut()
            .resource_mut::<TouchGestureCaptureState>()
            .capture(9, capture_owner);
        app.world_mut()
            .write_message(touch_input(TouchPhase::Moved, 9, Vec2::new(32.0, 10.0)));
        app.update();

        {
            let state = app.world().get::<ScrollSnapState>(container).unwrap();
            assert!(!state.touching);
            assert_eq!(state.active_touch, None);
            assert_eq!(state.velocity, 0.0);
            assert!(state.snap_anim.is_none());
        }

        app.world_mut()
            .write_message(touch_input(TouchPhase::Ended, 9, Vec2::new(32.0, 10.0)));
        app.update();

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert_eq!(state.velocity, 0.0);
        assert!(state.snap_anim.is_none());
    }

    #[test]
    fn capture_established_before_touch_end_without_move_does_not_trigger_snap() {
        let mut app = snap_touch_tracker_test_app();
        let (container, slider_like_child) = spawn_snap_container(&mut app, 40.0);

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 15, Vec2::new(10.0, 10.0)));
        app.update();

        {
            let state = app.world().get::<ScrollSnapState>(container).unwrap();
            assert!(state.touching);
            assert_eq!(state.active_touch, Some(15));
        }

        app.world_mut()
            .resource_mut::<TouchGestureCaptureState>()
            .capture(15, slider_like_child);
        app.world_mut()
            .write_message(touch_input(TouchPhase::Ended, 15, Vec2::new(10.0, 10.0)));
        app.update();

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert_eq!(state.velocity, 0.0);
        assert!(state.snap_anim.is_none());
    }

    #[test]
    fn uncaptured_touch_keeps_existing_snap_behavior() {
        let mut app = snap_touch_tracker_test_app();
        let (container, _) = spawn_snap_container(&mut app, 40.0);

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 11, Vec2::new(10.0, 10.0)));
        app.update();
        app.world_mut()
            .write_message(touch_input(TouchPhase::Ended, 11, Vec2::new(10.0, 10.0)));
        app.update();

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert!(state.snap_anim.is_some());
    }

    #[test]
    fn scroll_to_snap_requests_redraw_for_first_animation_frame() {
        let mut app = App::new();
        app.add_message::<RequestRedraw>()
            .add_observer(on_scroll_to_snap)
            .add_systems(bevy::prelude::PostUpdate, resolve_snap_requests);
        let (container, _) = spawn_snap_container(&mut app, 40.0);

        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Index(0),
            instant: false,
        });

        app.world_mut().flush();
        let mut reader = app.world_mut().resource_mut::<Messages<RequestRedraw>>().get_cursor();
        let redraw_count = reader.read(app.world().resource::<Messages<RequestRedraw>>()).count();

        assert_eq!(redraw_count, 1);
    }

    #[test]
    fn captured_touch_cancel_clears_tracking_without_snap() {
        let mut app = snap_touch_tracker_test_app();
        let (container, _) = spawn_snap_container(&mut app, 40.0);
        let capture_owner = app.world_mut().spawn_empty().id();

        app.world_mut()
            .write_message(touch_input(TouchPhase::Started, 13, Vec2::new(10.0, 10.0)));
        app.update();
        app.world_mut()
            .resource_mut::<TouchGestureCaptureState>()
            .capture(13, capture_owner);
        app.world_mut()
            .write_message(touch_input(TouchPhase::Canceled, 13, Vec2::new(10.0, 10.0)));
        app.update();

        let state = app.world().get::<ScrollSnapState>(container).unwrap();
        assert!(!state.touching);
        assert_eq!(state.active_touch, None);
        assert_eq!(state.velocity, 0.0);
        assert!(state.snap_anim.is_none());
    }

    #[test]
    fn explicit_snap_entity_and_declaration_index_distinguish_selection_from_layout() {
        let mut app = App::new();
        app.add_message::<RequestRedraw>()
            .add_observer(on_scroll_to_snap)
            .add_systems(
                bevy::prelude::PostUpdate,
                (resolve_snap_requests, observe_snap_status).chain(),
            );
        let (container, first) = spawn_snap_container(&mut app, 0.0);
        let second = app
            .world_mut()
            .spawn((
                ScrollSnapPoint,
                computed_node(Vec2::splat(100.0), Vec2::splat(100.0)),
                ui_transform(50.0, 50.0),
            ))
            .id();
        app.world_mut().entity_mut(container).add_child(second);
        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Entity(first),
            instant: true,
        });
        app.world_mut().flush();
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(container).unwrap().x, 100.0);
        assert_eq!(
            *app.world().get::<ScrollSnapStatus>(container).unwrap(),
            ScrollSnapStatus {
                selected: Some(first),
                settled: None
            }
        );
        app.world_mut()
            .get_mut::<ComputedNode>(container)
            .unwrap()
            .scroll_position
            .x = 100.0;
        app.world_mut().entity_mut(first).insert(ui_transform(50.0, 50.0));
        app.world_mut().entity_mut(second).insert(ui_transform(-50.0, 50.0));
        app.update();
        assert_eq!(
            app.world().get::<ScrollSnapStatus>(container).unwrap().settled,
            Some(first)
        );
        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Index(1),
            instant: true,
        });
        app.world_mut().flush();
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(container).unwrap().x, 0.0);
        let foreign = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Entity(foreign),
            instant: true,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<ScrollSnapStatus>(container).unwrap().selected,
            Some(second)
        );
    }

    #[test]
    fn explicit_short_tail_snap_retains_identity_at_the_native_limit() {
        let mut app = App::new();
        app.add_message::<RequestRedraw>()
            .add_observer(on_scroll_to_snap)
            .add_systems(
                bevy::prelude::PostUpdate,
                (resolve_snap_requests, observe_snap_status).chain(),
            );
        let (container, first) = spawn_snap_container(&mut app, 0.0);
        let tail = app
            .world_mut()
            .spawn((
                ScrollSnapPoint,
                computed_node(Vec2::splat(20.0), Vec2::splat(20.0)),
                ui_transform(190.0, 50.0),
            ))
            .id();
        app.world_mut().entity_mut(container).add_child(tail);
        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Entity(tail),
            instant: true,
        });
        app.world_mut().flush();
        app.world_mut()
            .get_mut::<ComputedNode>(container)
            .unwrap()
            .scroll_position
            .x = 100.0;
        app.world_mut().entity_mut(first).insert(ui_transform(50.0, 50.0));
        app.world_mut().entity_mut(tail).insert(ui_transform(90.0, 50.0));
        app.update();
        assert_eq!(
            *app.world().get::<ScrollSnapStatus>(container).unwrap(),
            ScrollSnapStatus {
                selected: Some(tail),
                settled: Some(tail)
            }
        );
    }

    #[test]
    fn snap_request_uses_geometry_after_the_same_frame_layout_change() {
        let mut app = App::new();
        app.add_message::<RequestRedraw>()
            .add_observer(on_scroll_to_snap)
            .add_systems(bevy::prelude::PostUpdate, resolve_snap_requests);
        let (container, point) = spawn_snap_container(&mut app, 0.0);
        app.world_mut().trigger(ScrollToSnap {
            entity: container,
            target: SnapTarget::Entity(point),
            instant: true,
        });
        app.world_mut().flush();
        // A responsive update changes layout before the next layout-dependent phase.
        app.world_mut()
            .get_mut::<ComputedNode>(container)
            .unwrap()
            .content_size
            .x = 300.0;
        app.world_mut().entity_mut(point).insert(ui_transform(250.0, 50.0));
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(container).unwrap().x, 200.0);
    }

    #[test]
    fn explicit_stop_does_not_restart_idle_resnap() {
        let mut app = snap_touch_tracker_test_app();
        app.init_resource::<crate::fling::FlingState>()
            .init_resource::<crate::overscroll::OverscrollBounceState>()
            .add_message::<RequestRedraw>()
            .add_observer(crate::on_stop_scroll)
            .add_systems(Update, scroll_snap_system);
        let (container, _) = spawn_snap_container(&mut app, 40.0);
        app.world_mut().trigger(crate::StopScroll { entity: container });
        app.world_mut().flush();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(container).unwrap().x, 40.0);
    }

    fn snap_touch_tracker_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<TouchScrollState>()
            .init_resource::<TouchGestureCaptureState>()
            .add_message::<TouchInput>()
            .add_systems(Update, scroll_snap_touch_tracker);
        app
    }

    fn scroll_and_snap_regression_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<TouchScrollState>()
            .init_resource::<TouchGestureCaptureState>()
            .init_resource::<super::super::fling::FlingState>()
            .init_resource::<HoverMap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_message::<MouseWheel>()
            .add_message::<TouchInput>()
            .add_systems(Update, (send_scroll_events, scroll_snap_touch_tracker).chain());
        app
    }

    fn spawn_snap_container(app: &mut App, current_scroll: f32) -> (Entity, Entity) {
        let container = app
            .world_mut()
            .spawn((
                horizontal_scroll_node(),
                ScrollPosition(Vec2::new(current_scroll, 0.0)),
                ScrollSnap {
                    axis: ScrollSnapAxis::X,
                },
                ScrollSnapState::default(),
                computed_node(Vec2::new(100.0, 100.0), Vec2::new(200.0, 100.0)),
                ui_transform(50.0, 50.0),
            ))
            .id();
        let snap_point = app
            .world_mut()
            .spawn((
                ScrollSnapPoint,
                ScrollGestureCapture,
                computed_node(Vec2::new(100.0, 100.0), Vec2::new(100.0, 100.0)),
                ui_transform(150.0, 50.0),
            ))
            .id();
        app.world_mut().entity_mut(container).add_child(snap_point);
        (container, snap_point)
    }

    fn horizontal_scroll_node() -> Node {
        Node {
            overflow: Overflow::scroll_x(),
            ..Default::default()
        }
    }

    fn insert_hover_hits(hover_map: &mut HoverMap, pointer_id: PointerId, entities: &[Entity]) {
        let bucket = hover_map.entry(pointer_id).or_default();
        for (index, entity) in entities.iter().enumerate() {
            bucket.insert(*entity, hit_data(index as f32));
        }
    }

    fn hit_data(depth: f32) -> HitData {
        HitData::new(Entity::PLACEHOLDER, depth, None, None)
    }

    fn computed_node(size: Vec2, content_size: Vec2) -> ComputedNode {
        let mut node = ComputedNode::default();
        node.size = size;
        node.content_size = content_size;
        node.unrounded_size = size;
        node.inverse_scale_factor = 1.0;
        node
    }

    fn ui_transform(x: f32, y: f32) -> UiGlobalTransform {
        UiGlobalTransform::from(Affine2::from_translation(Vec2::new(x, y)))
    }

    fn touch_input(phase: TouchPhase, id: u64, position: Vec2) -> TouchInput {
        TouchInput {
            phase,
            position,
            window: Entity::PLACEHOLDER,
            force: None,
            id,
        }
    }
}
