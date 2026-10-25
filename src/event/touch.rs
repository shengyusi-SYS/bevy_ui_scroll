//! 触摸滚动：Scroll Session、方向锁定、事件注入。

use std::collections::HashMap;

use bevy::{
    ecs::{
        entity::Entity,
        hierarchy::ChildOf,
        message::MessageReader,
        query::{Has, With},
        system::{Commands, Query, Res, ResMut},
    },
    input::{
        mouse::MouseScrollUnit,
        touch::{TouchInput, TouchPhase},
    },
    math::Vec2,
    picking::{hover::HoverMap, pointer::PointerId},
    time::Time,
    ui::{Node, OverflowAxis},
};

use super::super::fling::{FlingEntry, FlingState, spline_fling_distance, spline_fling_duration};
use super::super::gesture_capture::TouchGestureCaptureState;
use super::super::velocity::{FLING_MIN_VELOCITY, FlingVelocitySampler, clamp_fling_velocity};
use super::{DirectionLock, Scroll, ScrollSource};
use crate::pull_to_refresh::{PullRefreshControls, ScrollPullGesture};

/// Chrome Touch Slop (px): 累积滑动距离超过此阈值才开始滚动
const TOUCH_SLOP: f32 = 8.0;

/// Chrome "rail" 判断比例：主轴 > RAIL_RATIO × 次轴 才锁定方向
const RAIL_RATIO: f32 = 2.0;

// ── TouchScrollState ──

/// 单个触摸点的状态
pub(crate) struct TouchTrackState {
    prev_pos: Vec2,
    cumulative: Vec2,
    lock: DirectionLock,
    sampler: FlingVelocitySampler,
    /// Scroll Session: TouchDown 时暂存的 hover entities
    pending_targets: Vec<Entity>,
    /// Scroll Session: 方向锁定后确定的唯一滚动目标
    pub(crate) scroll_target: Option<Entity>,
}

/// 触摸滚动状态（Resource，供 snap tracker 读取方向锁定和 Scroll Session 信息）
#[derive(Default, bevy::ecs::resource::Resource)]
pub(crate) struct TouchScrollState {
    pub(crate) touches: HashMap<u64, TouchTrackState>,
    /// 已结束触摸的方向锁定快照（供同帧后续系统读取，下帧清理）
    ended_locks: HashMap<u64, DirectionLock>,
    /// 已结束触摸的 Scroll Session 目标快照（供同帧后续系统读取，下帧清理）
    ended_targets: HashMap<u64, Entity>,
}

impl TouchScrollState {
    /// 查询指定触摸 ID 的方向锁定状态（活跃或刚结束的）
    pub(crate) fn direction_lock(&self, touch_id: u64) -> Option<DirectionLock> {
        self.touches
            .get(&touch_id)
            .map(|t| t.lock)
            .or_else(|| self.ended_locks.get(&touch_id).copied())
    }

    /// 查询指定触摸 ID 的 Scroll Session 目标（活跃或刚结束的）
    pub(crate) fn scroll_target(&self, touch_id: u64) -> Option<Entity> {
        self.touches
            .get(&touch_id)
            .and_then(|t| t.scroll_target)
            .or_else(|| self.ended_targets.get(&touch_id).copied())
    }
}

// ── send_scroll_events 系统 ──

/// 注入鼠标滚轮和触屏滚动事件到 UI 层级。
pub(crate) fn send_scroll_events(
    mut touch_events: MessageReader<TouchInput>,
    mut touch_state: ResMut<TouchScrollState>,
    captures: Res<TouchGestureCaptureState>,
    mut fling_state: ResMut<FlingState>,
    hover_map: Res<HoverMap>,
    time: Res<Time>,
    node_q: Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
    parent_q: Query<&ChildOf>,
    refresh_targets: Query<(), With<PullRefreshControls>>,
    mut commands: Commands,
) {
    let base_time = time.elapsed_secs();

    // 清理上帧的已结束触摸快照
    touch_state.ended_locks.clear();
    touch_state.ended_targets.clear();

    // 触屏 — Scroll Session 模式
    for touch in touch_events.read() {
        match touch.phase {
            TouchPhase::Started => {
                let pending = pending_targets_for_touch(&hover_map, touch.id);
                // Pressing a viewport takes over old motion before crossing slop.
                let mut owners = std::collections::HashSet::new();
                for &hit in &pending {
                    for entity in std::iter::once(hit).chain(parent_q.iter_ancestors::<ChildOf>(hit)) {
                        if let Ok((_, enhanced, native)) = node_q.get(entity) {
                            if native {
                                break;
                            }
                            if enhanced {
                                owners.insert(entity);
                            }
                        }
                    }
                }
                for entity in owners {
                    commands.trigger(crate::StopScroll { entity });
                    commands.trigger(crate::ScrollInput {
                        entity,
                        kind: crate::ScrollInputKind::Touch,
                    });
                }
                // §3: 取消与当前触摸区域重叠的 fling
                fling_state
                    .entries
                    .retain(|entry| !is_ancestor_of_any(entry.target, &pending, &parent_q));
                let mut sampler = FlingVelocitySampler::default();
                sampler.add(base_time, touch.position);
                touch_state.touches.insert(
                    touch.id,
                    TouchTrackState {
                        prev_pos: touch.position,
                        cumulative: Vec2::ZERO,
                        lock: DirectionLock::None,
                        sampler,
                        pending_targets: pending,
                        scroll_target: None,
                    },
                );
            }
            TouchPhase::Moved => {
                if let Some(track) = touch_state.touches.get_mut(&touch.id) {
                    let raw_delta = track.prev_pos - touch.position;
                    track.prev_pos = touch.position;

                    track.sampler.add(base_time, touch.position);

                    // 方向锁定
                    let was_locked = track.lock != DirectionLock::None;
                    if track.lock == DirectionLock::None {
                        track.cumulative += raw_delta;
                        let dist = track.cumulative.length();
                        if dist > TOUCH_SLOP {
                            let ax = track.cumulative.x.abs();
                            let ay = track.cumulative.y.abs();
                            track.lock = if ax > RAIL_RATIO * ay {
                                DirectionLock::Horizontal
                            } else if ay > RAIL_RATIO * ax {
                                DirectionLock::Vertical
                            } else {
                                DirectionLock::Free
                            };

                            track.scroll_target = resolve_scroll_target(
                                touch.id,
                                &track.pending_targets,
                                track.lock,
                                &captures,
                                &node_q,
                                &parent_q,
                            );
                            track.pending_targets.clear();
                        }
                    }

                    if track.lock == DirectionLock::None {
                        continue;
                    }

                    let delta = if !was_locked {
                        match track.lock {
                            DirectionLock::Horizontal => Vec2::new(track.cumulative.x, 0.0),
                            DirectionLock::Vertical => Vec2::new(0.0, track.cumulative.y),
                            DirectionLock::Free => track.cumulative,
                            DirectionLock::None => unreachable!(),
                        }
                    } else {
                        match track.lock {
                            DirectionLock::Horizontal => Vec2::new(raw_delta.x, 0.0),
                            DirectionLock::Vertical => Vec2::new(0.0, raw_delta.y),
                            DirectionLock::Free => raw_delta,
                            DirectionLock::None => unreachable!(),
                        }
                    };

                    if let Some(target) = track.scroll_target {
                        commands.trigger(Scroll {
                            entity: target,
                            delta,
                            source: ScrollSource::Touch,
                            original_delta: delta,
                            unit: MouseScrollUnit::Pixel,
                        });
                        if refresh_targets.contains(target) {
                            commands.trigger(ScrollPullGesture {
                                entity: target,
                                touch: touch.id,
                                phase: if captures.owner(touch.id).is_some() {
                                    TouchPhase::Canceled
                                } else {
                                    TouchPhase::Moved
                                },
                                vertical: track.lock == DirectionLock::Vertical,
                            });
                        }
                    }
                }
            }
            TouchPhase::Ended => {
                if let Some(track) = touch_state.touches.remove(&touch.id) {
                    touch_state.ended_locks.insert(touch.id, track.lock);
                    if let Some(target) = track.scroll_target {
                        touch_state.ended_targets.insert(touch.id, target);
                        if refresh_targets.contains(target) {
                            commands.trigger(ScrollPullGesture {
                                entity: target,
                                touch: touch.id,
                                phase: if captures.owner(touch.id).is_some() {
                                    TouchPhase::Canceled
                                } else {
                                    TouchPhase::Ended
                                },
                                vertical: track.lock == DirectionLock::Vertical,
                            });
                        }
                    }
                    if track.lock == DirectionLock::None {
                        continue;
                    }
                    let Some(target) = track.scroll_target else {
                        continue;
                    };
                    let raw_velocity = -track.sampler.velocity(base_time);
                    let velocity = match track.lock {
                        DirectionLock::Horizontal => Vec2::new(raw_velocity.x, 0.0),
                        DirectionLock::Vertical => Vec2::new(0.0, raw_velocity.y),
                        DirectionLock::Free => raw_velocity,
                        DirectionLock::None => unreachable!(),
                    };
                    let velocity = clamp_fling_velocity(velocity);

                    if velocity.length() > FLING_MIN_VELOCITY {
                        let dist_x = spline_fling_distance(velocity.x as f64) as f32 * velocity.x.signum();
                        let dist_y = spline_fling_distance(velocity.y as f64) as f32 * velocity.y.signum();
                        let dur_x = spline_fling_duration(velocity.x as f64) as f32;
                        let dur_y = spline_fling_duration(velocity.y as f64) as f32;
                        bevy::log::trace!(
                            "[fling] Created: vel={velocity:?} dist=({dist_x:.1},{dist_y:.1}) dur=({dur_x:.3},{dur_y:.3})s target={target:?}",
                        );
                        fling_state.entries.push(FlingEntry {
                            target,
                            total_distance: Vec2::new(dist_x, dist_y),
                            duration: Vec2::new(dur_x, dur_y),
                            elapsed: 0.0,
                            prev_position_fraction: Vec2::ZERO,
                        });
                    } else {
                        bevy::log::trace!(
                            "[fling] Rejected: vel={velocity:?} len={:.1} < MIN={FLING_MIN_VELOCITY}",
                            velocity.length()
                        );
                    }
                }
            }
            TouchPhase::Canceled => {
                if let Some(track) = touch_state.touches.remove(&touch.id)
                    && let Some(target) = track.scroll_target
                    && refresh_targets.contains(target)
                {
                    commands.trigger(ScrollPullGesture {
                        entity: target,
                        touch: touch.id,
                        phase: TouchPhase::Canceled,
                        vertical: track.lock == DirectionLock::Vertical,
                    });
                }
            }
        }
    }
}

// ── Scroll Session 辅助函数 ──

fn pending_targets_for_touch(hover_map: &HoverMap, touch_id: u64) -> Vec<Entity> {
    hover_map
        .get(&PointerId::Touch(touch_id))
        .map(|pointer_hits| pointer_hits.keys().copied().collect())
        .unwrap_or_default()
}

fn resolve_scroll_target(
    touch_id: u64,
    pending_targets: &[Entity],
    lock: DirectionLock,
    captures: &TouchGestureCaptureState,
    node_q: &Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
    parent_q: &Query<&ChildOf>,
) -> Option<Entity> {
    if captures.owner(touch_id).is_some() {
        return None;
    }

    select_scroll_target(pending_targets, lock, node_q, parent_q)
}

/// 从 hover entity 出发，沿 ECS 层级向上查找最内层可滚动容器。
fn select_scroll_target(
    candidates: &[Entity],
    lock: DirectionLock,
    node_q: &Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
    parent_q: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut best: Option<(Entity, u32)> = None;

    for &candidate in candidates {
        let mut current = candidate;
        let mut depth = entity_depth(candidate, parent_q);
        loop {
            if node_q.get(current).is_ok_and(|(_, _, native)| native) {
                break;
            }
            if is_scrollable(current, lock, node_q) {
                match best {
                    Some((_, best_depth)) if depth <= best_depth => {}
                    _ => best = Some((current, depth)),
                }
                break;
            }
            if let Ok(child_of) = parent_q.get(current) {
                current = child_of.0;
                depth = depth.saturating_sub(1);
            } else {
                break;
            }
        }
    }

    best.map(|(e, _)| e)
}

/// 检查 entity 是否在指定方向上可滚动
fn is_scrollable(
    entity: Entity,
    lock: DirectionLock,
    node_q: &Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
) -> bool {
    let Ok((node, true, false)) = node_q.get(entity) else {
        return false;
    };
    match lock {
        DirectionLock::Horizontal => node.overflow.x == OverflowAxis::Scroll,
        DirectionLock::Vertical => node.overflow.y == OverflowAxis::Scroll,
        DirectionLock::Free => node.overflow.x == OverflowAxis::Scroll || node.overflow.y == OverflowAxis::Scroll,
        DirectionLock::None => false,
    }
}

/// 检查 ancestor 是否是 candidates 中任意 entity 的祖先（或自身）
fn is_ancestor_of_any(ancestor: Entity, candidates: &[Entity], parent_q: &Query<&ChildOf>) -> bool {
    candidates.iter().any(|&candidate| {
        let mut current = candidate;
        loop {
            if current == ancestor {
                return true;
            }
            match parent_q.get(current) {
                Ok(child_of) => current = child_of.0,
                Err(_) => return false,
            }
        }
    })
}

/// 计算 entity 在 ECS 层级中的深度
pub(crate) fn entity_depth(entity: Entity, parent_q: &Query<&ChildOf>) -> u32 {
    parent_q.iter_ancestors::<ChildOf>(entity).count() as u32
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use bevy::{
        ecs::system::SystemState,
        input::{
            ButtonInput,
            keyboard::KeyCode,
            mouse::MouseWheel,
            touch::{TouchInput, TouchPhase},
        },
        picking::backend::HitData,
        prelude::*,
        ui::Overflow,
    };

    use super::*;

    #[derive(Resource, Default)]
    struct ScrollEventCount(usize);

    fn count_scroll_events(_: On<super::Scroll>, mut count: ResMut<ScrollEventCount>) {
        count.0 += 1;
    }

    fn hit_data(depth: f32) -> HitData {
        HitData::new(Entity::PLACEHOLDER, depth, None, None)
    }

    fn insert_hover_hits(hover_map: &mut HoverMap, pointer_id: PointerId, entities: &[Entity]) {
        let bucket = hover_map.entry(pointer_id).or_default();
        for (index, entity) in entities.iter().enumerate() {
            bucket.insert(*entity, hit_data(index as f32));
        }
    }

    fn vertical_scroll_node() -> Node {
        Node {
            overflow: Overflow::scroll_y(),
            ..Default::default()
        }
    }

    #[test]
    fn started_touch_collects_only_its_own_hover_candidates() {
        let mut world = World::new();
        let touch_entity_a = world.spawn_empty().id();
        let touch_entity_b = world.spawn_empty().id();
        let other_touch_entity = world.spawn_empty().id();

        let mut hover_map = HoverMap::default();
        insert_hover_hits(&mut hover_map, PointerId::Touch(7), &[touch_entity_a, touch_entity_b]);
        insert_hover_hits(&mut hover_map, PointerId::Touch(8), &[other_touch_entity]);

        let pending = pending_targets_for_touch(&hover_map, 7);
        let pending_set: HashSet<_> = pending.into_iter().collect();

        assert_eq!(pending_set, HashSet::from([touch_entity_a, touch_entity_b]));
    }

    #[test]
    fn started_touch_ignores_other_pointer_hover_candidates() {
        let mut world = World::new();
        let own_touch_entity = world.spawn_empty().id();
        let other_touch_entity = world.spawn_empty().id();
        let mouse_entity = world.spawn_empty().id();

        let mut hover_map = HoverMap::default();
        insert_hover_hits(&mut hover_map, PointerId::Touch(11), &[own_touch_entity]);
        insert_hover_hits(&mut hover_map, PointerId::Touch(12), &[other_touch_entity]);
        insert_hover_hits(&mut hover_map, PointerId::Mouse, &[mouse_entity]);

        let pending = pending_targets_for_touch(&hover_map, 11);

        assert_eq!(pending, vec![own_touch_entity]);
    }

    #[test]
    fn captured_touch_skips_scroll_target_selection() {
        let mut world = World::new();
        let root = world.spawn((vertical_scroll_node(), crate::EnhancedScroll)).id();
        let inner = world
            .spawn((vertical_scroll_node(), crate::EnhancedScroll, ChildOf(root)))
            .id();
        let leaf = world.spawn(ChildOf(inner)).id();

        let mut captures = TouchGestureCaptureState::default();
        captures.capture(5, leaf);

        let mut system_state: SystemState<(
            Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
            Query<&ChildOf>,
        )> = SystemState::new(&mut world);
        let Ok((node_q, parent_q)) = system_state.get_mut(&mut world) else {
            panic!("touch scroll test queries should be valid");
        };

        let target = resolve_scroll_target(5, &[leaf], DirectionLock::Vertical, &captures, &node_q, &parent_q);

        assert_eq!(target, None);
    }

    #[test]
    fn uncaptured_touch_still_selects_deepest_scrollable_ancestor() {
        let mut world = World::new();
        let root = world.spawn((vertical_scroll_node(), crate::EnhancedScroll)).id();
        let inner = world
            .spawn((vertical_scroll_node(), crate::EnhancedScroll, ChildOf(root)))
            .id();
        let leaf = world.spawn(ChildOf(inner)).id();

        let captures = TouchGestureCaptureState::default();

        let mut system_state: SystemState<(
            Query<(&Node, Has<crate::EnhancedScroll>, Has<bevy::ui_widgets::ScrollArea>)>,
            Query<&ChildOf>,
        )> = SystemState::new(&mut world);
        let Ok((node_q, parent_q)) = system_state.get_mut(&mut world) else {
            panic!("touch scroll test queries should be valid");
        };

        let target = resolve_scroll_target(6, &[leaf], DirectionLock::Vertical, &captures, &node_q, &parent_q);

        assert_eq!(target, Some(inner));
    }

    #[test]
    fn captured_touch_does_not_emit_scroll_delta() {
        let mut app = App::new();
        app.init_resource::<TouchScrollState>()
            .init_resource::<TouchGestureCaptureState>()
            .init_resource::<FlingState>()
            .init_resource::<HoverMap>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Time>()
            .init_resource::<ScrollEventCount>()
            .add_message::<MouseWheel>()
            .add_message::<TouchInput>()
            .add_systems(Update, send_scroll_events)
            .add_observer(count_scroll_events);

        let window = app.world_mut().spawn_empty().id();
        let scroll_parent = app
            .world_mut()
            .spawn((vertical_scroll_node(), crate::EnhancedScroll))
            .id();
        let captured_leaf = app.world_mut().spawn(ChildOf(scroll_parent)).id();

        {
            let mut hover_map = app.world_mut().resource_mut::<HoverMap>();
            insert_hover_hits(&mut hover_map, PointerId::Touch(9), &[captured_leaf]);
        }
        {
            let mut captures = app.world_mut().resource_mut::<TouchGestureCaptureState>();
            captures.capture(9, captured_leaf);
        }

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Started,
            position: Vec2::ZERO,
            window,
            force: None,
            id: 9,
        });
        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Moved,
            position: Vec2::new(0.0, -16.0),
            window,
            force: None,
            id: 9,
        });

        app.update();

        let touch_state = app.world().resource::<TouchScrollState>();
        let track = touch_state
            .touches
            .get(&9)
            .expect("touch track should remain active after move");

        assert!(matches!(track.lock, DirectionLock::Vertical));
        assert_eq!(track.scroll_target, None);
        assert_eq!(app.world().resource::<ScrollEventCount>().0, 0);
    }
}
