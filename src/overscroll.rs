//! 普通容器 overscroll 回弹系统。
//!
//! 当触摸释放且 ScrollPosition 越过边界时，使用 critically damped spring 回弹。
//! OverscrollBounceState 只负责未由 snap 接管的轴。

use bevy::{
    ecs::{
        entity::Entity,
        message::MessageWriter,
        system::{Query, Res, ResMut},
    },
    time::Time,
    ui::{ComputedNode, Node, OverflowAxis, ScrollPosition},
    window::RequestRedraw,
};

use super::event::TouchScrollState;
use super::fling::FlingState;
use super::snap::{ScrollSnap, ScrollSnapAxis};

/// Return a completed pull through the existing spring, including batched
/// start/move/end input that never leaves an active touch for the bounce system.
pub(crate) fn release_refresh_pull(world: &mut bevy::prelude::World, target: Entity) {
    if world
        .get::<ScrollSnap>(target)
        .is_some_and(|snap| snap.axis == ScrollSnapAxis::Y)
    {
        return;
    }
    world
        .resource_mut::<FlingState>()
        .entries
        .retain(|entry| entry.target != target);
    let Some(position) = world.get::<ScrollPosition>(target).map(|position| position.y) else {
        return;
    };
    if position >= 0.0 {
        return;
    }
    let mut bounce = world.resource_mut::<OverscrollBounceState>();
    let entry = bounce.entries.entry(target).or_insert(OverscrollEntry {
        spring_x: None,
        spring_y: None,
        was_touching: false,
    });
    entry.spring_y = Some(OverscrollSpring::new(position, 0.0, 0.0));
}

// ── Overscroll 常量 ──

/// 普通容器触摸 overscroll 阻尼系数
pub(super) const NORMAL_OVERSCROLL_RESISTANCE: f32 = 0.3;
/// 普通容器 overscroll 最大弹性距离 (px)
pub(super) const NORMAL_MAX_OVERSCROLL: f32 = 150.0;
/// Overscroll spring 自然频率 ω (rad/s)
const OVERSCROLL_SPRING_OMEGA: f32 = 20.0;
/// Overscroll spring 到位阈值 (px)
const OVERSCROLL_SNAP_THRESHOLD: f32 = 0.5;

// ── Overscroll Spring ──

/// Critically damped spring 用于 overscroll 回弹。
///
/// 解析解：x(t) = target + (A + B*t) * e^(-ω*t)
/// 其中 A = start - target, B = v₀ + ω*A
pub(super) struct OverscrollSpring {
    start: f32,
    pub(super) target: f32,
    velocity: f32,
    elapsed: f32,
    omega: f32,
}

impl OverscrollSpring {
    pub(super) fn new(start: f32, target: f32, initial_velocity: f32) -> Self {
        Self {
            start,
            target,
            velocity: initial_velocity,
            elapsed: 0.0,
            omega: OVERSCROLL_SPRING_OMEGA,
        }
    }

    /// 返回 (新位置, 新速度, 是否完成)
    pub(super) fn update(&mut self, dt: f32) -> (f32, f32, bool) {
        self.elapsed += dt;
        let a = self.start - self.target;
        let b = self.velocity + self.omega * a;
        let (displacement, vel) = crate::motion::spring_sample(a, b, self.omega, self.elapsed);
        let pos = self.target + displacement;
        let done = (pos - self.target).abs() < OVERSCROLL_SNAP_THRESHOLD && vel.abs() < OVERSCROLL_SNAP_THRESHOLD;
        (pos, vel, done)
    }
}

/// 单个容器的 overscroll 回弹条目
pub(super) struct OverscrollEntry {
    pub(super) spring_x: Option<OverscrollSpring>,
    pub(super) spring_y: Option<OverscrollSpring>,
    pub(super) was_touching: bool,
}

/// 全局 overscroll 回弹状态（Resource）
#[derive(Default, bevy::ecs::resource::Resource)]
pub(super) struct OverscrollBounceState {
    pub(super) entries: std::collections::HashMap<Entity, OverscrollEntry>,
}

/// 普通容器 overscroll 回弹系统。
///
/// 工作流程：
/// - `on_scroll_handler` 在触摸拖动时清除 spring（手指接管）
/// - `touch_fling_system` 在 fling 碰到边界时创建 spring
/// - 此系统检测触摸释放（拖动到越界后松手无 fling），并推动 spring 动画
pub(super) fn overscroll_bounce_system(
    mut scroll_q: Query<
        (Entity, &mut ScrollPosition, &Node, &ComputedNode, Option<&ScrollSnap>),
        bevy::prelude::With<crate::EnhancedScroll>,
    >,
    touch_state: Res<TouchScrollState>,
    mut bounce_state: ResMut<OverscrollBounceState>,
    fling_state: Res<FlingState>,
    time: Res<Time>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    // 收集当前被活跃触摸指向的 entity 集合
    let touching_entities: std::collections::HashSet<Entity> =
        touch_state.touches.values().filter_map(|t| t.scroll_target).collect();

    // 收集当前有活跃 fling 的 entity 集合
    let flinging_entities: std::collections::HashSet<Entity> = fling_state.entries.iter().map(|e| e.target).collect();

    // 遍历所有 scroll 容器，检测触摸释放 → 越界 → 启动 spring
    for (entity, scroll_pos, node, computed, snap) in scroll_q.iter() {
        let is_touching = touching_entities.contains(&entity);
        let entry = bounce_state.entries.get(&entity);
        let was_touching = entry.is_some_and(|e| e.was_touching);

        // 触摸释放瞬间：was_touching=true → is_touching=false
        if was_touching && !is_touching && !flinging_entities.contains(&entity) {
            let max_offset = crate::max_scroll_offset(computed);
            let mut need_spring_x = None;
            let mut need_spring_y = None;

            if node.overflow.x == OverflowAxis::Scroll && snap.is_none_or(|snap| snap.axis != ScrollSnapAxis::X) {
                let max_x = max_offset.x.max(0.0);
                if scroll_pos.x < 0.0 {
                    need_spring_x = Some(OverscrollSpring::new(scroll_pos.x, 0.0, 0.0));
                } else if scroll_pos.x > max_x {
                    need_spring_x = Some(OverscrollSpring::new(scroll_pos.x, max_x, 0.0));
                }
            }
            if node.overflow.y == OverflowAxis::Scroll && snap.is_none_or(|snap| snap.axis != ScrollSnapAxis::Y) {
                let max_y = max_offset.y.max(0.0);
                if scroll_pos.y < 0.0 {
                    need_spring_y = Some(OverscrollSpring::new(scroll_pos.y, 0.0, 0.0));
                } else if scroll_pos.y > max_y {
                    need_spring_y = Some(OverscrollSpring::new(scroll_pos.y, max_y, 0.0));
                }
            }

            if need_spring_x.is_some() || need_spring_y.is_some() {
                let bounce_entry = bounce_state.entries.entry(entity).or_insert_with(|| OverscrollEntry {
                    spring_x: None,
                    spring_y: None,
                    was_touching: false,
                });
                if need_spring_x.is_some() {
                    bounce_entry.spring_x = need_spring_x;
                }
                if need_spring_y.is_some() {
                    bounce_entry.spring_y = need_spring_y;
                }
            }
        }
    }

    // 更新所有 entry 的 was_touching 状态
    let all_entities: Vec<Entity> = bounce_state.entries.keys().copied().collect();
    for entity in &all_entities {
        if let Some(entry) = bounce_state.entries.get_mut(entity) {
            entry.was_touching = touching_entities.contains(entity);
        }
    }
    // 对于正在被触摸但还没有 entry 的容器，创建 entry 追踪 was_touching
    for &entity in &touching_entities {
        if !bounce_state.entries.contains_key(&entity) {
            bounce_state.entries.insert(
                entity,
                OverscrollEntry {
                    spring_x: None,
                    spring_y: None,
                    was_touching: true,
                },
            );
        }
    }

    // 推进 spring 动画 + 清理已完成条目
    bounce_state.entries.retain(|entity, entry| {
        let Ok((_, mut scroll_pos, _, _, snap)) = scroll_q.get_mut(*entity) else {
            return false;
        };
        // A snap axis has its own boundary return. Also retire stale springs if
        // the component/axis changed while ordinary bounce was in flight.
        if let Some(snap) = snap {
            match snap.axis {
                ScrollSnapAxis::X => entry.spring_x = None,
                ScrollSnapAxis::Y => entry.spring_y = None,
            }
        }
        let mut active = false;
        if let Some(spring) = entry.spring_x.as_mut() {
            let (pos, _vel, done) = spring.update(dt);
            scroll_pos.x = if done { spring.target } else { pos };
            if done {
                entry.spring_x = None;
            } else {
                active = true;
            }
        }
        if let Some(spring) = entry.spring_y.as_mut() {
            let (pos, _vel, done) = spring.update(dt);
            scroll_pos.y = if done { spring.target } else { pos };
            if done {
                entry.spring_y = None;
            } else {
                active = true;
            }
        }

        // 保留：有活跃 spring 或正在被触摸（追踪 was_touching）
        active || entry.was_touching
    });

    // 有活跃 springs → 请求下一帧重绘
    if bounce_state
        .entries
        .values()
        .any(|e| e.spring_x.is_some() || e.spring_y.is_some())
    {
        redraw.write(RequestRedraw);
    }
}
