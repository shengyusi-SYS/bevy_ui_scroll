//! Snap geometry helpers: spring animation, snap point collection, spatial queries.

use bevy::{
    ecs::{entity::Entity, query::With, system::Query},
    math::Vec2,
    prelude::Children,
    ui::{ComputedNode, ScrollPosition, UiGlobalTransform},
};

use super::{MIN_FLING_VELOCITY, SNAP_THRESHOLD, ScrollSnap, ScrollSnapAxis, ScrollSnapPoint};

// ── Snap 动画 ──

/// Critically damped spring 动画
///
/// 解析解：x(t) = target + (A + B*t) * e^(-ω*t)
/// 其中 A = start - target, B = v₀ + ω*A
pub(super) struct SpringAnim {
    pub(super) target: f32,
    omega: f32,
    a: f32,
    b: f32,
    elapsed: f32,
}

impl SpringAnim {
    pub(super) fn new(start: f32, target: f32, initial_velocity: f32, omega: f32) -> Self {
        let a = start - target;
        let b = initial_velocity + omega * a;
        Self {
            target,
            omega,
            a,
            b,
            elapsed: 0.0,
        }
    }

    /// 返回 (新位置, 新速度, 是否完成)
    pub(super) fn update(&mut self, dt: f32) -> (f32, f32, bool) {
        self.elapsed += dt;
        let (displacement, vel) = crate::motion::spring_sample(self.a, self.b, self.omega, self.elapsed);
        let pos = self.target + displacement;
        let done = (pos - self.target).abs() < SNAP_THRESHOLD && vel.abs() < MIN_FLING_VELOCITY;
        (pos, vel, done)
    }
}

// ── Snap 点收集与查询 ──

/// 基于 layout 引擎计算的实际位置收集 snap 点（逻辑像素）。
///
/// Chrome 标准：`SnapContainerData::UpdateSnapAreaOffsets` 使用元素的实际布局位置。
/// 本实现通过 `UiGlobalTransform` 读取子元素的全局位置，反算相对于父容器 content 区域的偏移。
/// 子元素的全局位置已包含 scroll 偏移，通过 `ComputedNode.scroll_position`（上一帧 layout 的
/// 物理 scroll offset）补偿，确保结果不依赖当前帧对 ScrollPosition 的修改。
pub(super) fn collect_snap_points(
    snap_axis: ScrollSnapAxis,
    parent_entity: Entity,
    parent_gt: &UiGlobalTransform,
    parent_computed: &ComputedNode,
    children_q: &Query<&Children>,
    snap_child_q: &Query<(&UiGlobalTransform, &ComputedNode), With<ScrollSnapPoint>>,
    snap_container_q: &Query<(), With<ScrollSnap>>,
) -> Vec<f32> {
    collect_snap_targets(
        snap_axis,
        parent_entity,
        parent_gt,
        parent_computed,
        children_q,
        snap_child_q,
        snap_container_q,
    )
    .into_iter()
    .map(|(_, offset)| offset)
    .collect()
}

pub(super) fn collect_snap_targets(
    snap_axis: ScrollSnapAxis,
    parent_entity: Entity,
    parent_gt: &UiGlobalTransform,
    parent_computed: &ComputedNode,
    children_q: &Query<&Children>,
    snap_child_q: &Query<(&UiGlobalTransform, &ComputedNode), With<ScrollSnapPoint>>,
    snap_container_q: &Query<(), With<ScrollSnap>>,
) -> Vec<(Entity, f32)> {
    let mut points = Vec::new();
    let isf = parent_computed.inverse_scale_factor();
    let content_inset = parent_computed.content_inset();

    // 上一帧 layout pass 中实际使用的物理 scroll offset
    let layout_scroll = match snap_axis {
        ScrollSnapAxis::X => parent_computed.scroll_position.x,
        ScrollSnapAxis::Y => parent_computed.scroll_position.y,
    };

    // 父容器 content 区域起始边（全局物理坐标）
    let parent_content_start = match snap_axis {
        ScrollSnapAxis::X => parent_gt.translation.x - parent_computed.size().x * 0.5 + content_inset.min_inset.x,
        ScrollSnapAxis::Y => parent_gt.translation.y - parent_computed.size().y * 0.5 + content_inset.min_inset.y,
    };

    // 递归遍历所有后代，收集带 ScrollSnapPoint 的节点（Chrome SnapAreaList 语义）
    // Reverse the stack push so visits follow Children declaration order.
    let mut stack: Vec<&Entity> = Vec::new();
    if let Ok(children) = children_q.get(parent_entity) {
        stack.extend(children.iter().rev());
    }
    while let Some(entity) = stack.pop() {
        // 嵌套 snap 容器：跳过整个子树（不入栈 children），
        // 其内部 ScrollSnapPoint 归属于嵌套容器而非当前容器（Chrome SnapContainerData 语义）
        if *entity != parent_entity && snap_container_q.contains(*entity) {
            continue;
        }
        if let Ok((child_gt, child_computed)) = snap_child_q.get(*entity) {
            let child_size = match snap_axis {
                ScrollSnapAxis::X => child_computed.size().x,
                ScrollSnapAxis::Y => child_computed.size().y,
            };
            // 子元素起始边（全局物理坐标），已受 scroll 影响
            let child_start = match snap_axis {
                ScrollSnapAxis::X => child_gt.translation.x - child_size * 0.5,
                ScrollSnapAxis::Y => child_gt.translation.y - child_size * 0.5,
            };
            // 反算 un-scrolled 偏移 = (child_start - parent_start) + layout_scroll
            // 转为逻辑像素
            let snap_point = (child_start - parent_content_start + layout_scroll) * isf;
            points.push((
                *entity,
                snap_point.clamp(0.0, compute_max_scroll(snap_axis, parent_computed)),
            ));
        }
        // 继续遍历该节点的子节点
        if let Ok(grandchildren) = children_q.get(*entity) {
            stack.extend(grandchildren.iter().rev());
        }
    }
    points
}

pub(super) fn nearest_snap_point(points: &[f32], current: f32) -> Option<f32> {
    points.iter().copied().min_by(|a, b| {
        let da = (a - current).abs();
        let db = (b - current).abs();
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

#[cfg(test)]
pub(super) fn nearest_snap_index(points: &[f32], pos: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (*a - pos).abs();
            let db = (*b - pos).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// 带方向偏好的 snap 点选择（Chrome aligned，无 magic number）。
///
/// Chrome `SnapFlingController::GetSnapFlingInfoAndSetTarget` 逻辑：
/// 1. 在 predicted_end 处找全局最近 snap 点
/// 2. 如果全局最近在 fling 方向上（超出当前 snap 点），直接选中
/// 3. 如果全局最近违反方向，用方向过滤后的最近点
/// 4. 方向无候选时回退到 current 最近（保持不动）
pub(super) fn directional_snap_point(points: &[f32], predicted_end: f32, current: f32, direction: f32) -> Option<f32> {
    if points.is_empty() {
        return None;
    }

    // 1. 全局最近——在 predicted_end 处
    let global_nearest = nearest_snap_point(points, predicted_end)?;

    // 无方向偏好（direction ≈ 0）→ 直接用全局最近
    if direction.abs() <= f32::EPSILON {
        return Some(global_nearest);
    }

    // 2. 找当前所在的 snap 点，作为方向过滤的基准
    let current_snap = nearest_snap_point(points, current)?;

    // 3. 检查全局最近是否在正确方向上
    let global_in_direction = if direction > 0.0 {
        global_nearest > current_snap + SNAP_THRESHOLD
    } else {
        global_nearest < current_snap - SNAP_THRESHOLD
    };

    if global_in_direction {
        return Some(global_nearest);
    }

    // 4. 全局最近违反方向 → 方向过滤后找最近
    let directional: Vec<f32> = points
        .iter()
        .copied()
        .filter(|&p| {
            if direction > 0.0 {
                p > current_snap + SNAP_THRESHOLD
            } else {
                p < current_snap - SNAP_THRESHOLD
            }
        })
        .collect();

    if let Some(dir_nearest) = nearest_snap_point(&directional, predicted_end) {
        return Some(dir_nearest);
    }

    // 5. 方向无候选 → 回退到 current 最近（保持不动）
    nearest_snap_point(points, current)
}

pub(super) fn compute_max_scroll(axis: ScrollSnapAxis, computed: &ComputedNode) -> f32 {
    let max = crate::max_scroll_offset(computed);
    match axis {
        ScrollSnapAxis::X => max.x,
        ScrollSnapAxis::Y => max.y,
    }
}

pub(super) fn get_scroll(scroll: &ScrollPosition, axis: ScrollSnapAxis) -> f32 {
    match axis {
        ScrollSnapAxis::X => scroll.x,
        ScrollSnapAxis::Y => scroll.y,
    }
}

pub(super) fn set_scroll(scroll: &mut ScrollPosition, axis: ScrollSnapAxis, val: f32) {
    match axis {
        ScrollSnapAxis::X => scroll.x = val,
        ScrollSnapAxis::Y => scroll.y = val,
    }
}

pub(super) fn get_touch_axis(position: Vec2, axis: ScrollSnapAxis) -> f32 {
    match axis {
        ScrollSnapAxis::X => position.x,
        ScrollSnapAxis::Y => position.y,
    }
}

/// D5: 判断屏幕坐标 point（逻辑像素）是否在 UI 节点的全局矩形内。
pub(super) fn point_in_node(point: Vec2, computed: &ComputedNode, global_transform: &UiGlobalTransform) -> bool {
    let isf = computed.inverse_scale_factor();
    let node_size = computed.size() * isf;
    let node_center = global_transform.translation * isf; // 物理→逻辑
    let half = node_size * 0.5;
    point.x >= node_center.x - half.x
        && point.x <= node_center.x + half.x
        && point.y >= node_center.y - half.y
        && point.y <= node_center.y + half.y
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::super::SNAP_SPRING_OMEGA;
    use super::*;

    // ══════════════════════════════════════════════
    // SpringAnim（对应 Chrome snap_fling_curve_unittest.cc）
    // ══════════════════════════════════════════════

    /// 零速度 + 起始=目标 → 立即完成
    #[test]
    fn spring_at_target() {
        let mut spring = SpringAnim::new(100.0, 100.0, 0.0, SNAP_SPRING_OMEGA);
        let (pos, vel, done) = spring.update(0.016);
        assert!((pos - 100.0).abs() < SNAP_THRESHOLD);
        assert!(vel.abs() < MIN_FLING_VELOCITY);
        assert!(done);
    }

    /// 零初速 spring 收敛到目标
    #[test]
    fn spring_converges_to_target() {
        let mut spring = SpringAnim::new(0.0, 300.0, 0.0, SNAP_SPRING_OMEGA);
        // 模拟 60fps，最多 300 帧（5s）
        for _ in 0..300 {
            let (pos, _vel, done) = spring.update(1.0 / 60.0);
            if done {
                assert!((pos - 300.0).abs() < SNAP_THRESHOLD);
                return;
            }
        }
        panic!("spring did not converge within 5s");
    }

    /// 带初速 spring 收敛到目标
    #[test]
    fn spring_with_velocity_converges() {
        let mut spring = SpringAnim::new(0.0, 300.0, 2000.0, SNAP_SPRING_OMEGA);
        for _ in 0..300 {
            let (_pos, _vel, done) = spring.update(1.0 / 60.0);
            if done {
                return;
            }
        }
        panic!("spring did not converge within 5s");
    }

    /// 半程位移在 start 和 target 之间（无过冲测试 — critically damped 允许微量过冲）
    #[test]
    fn spring_midpoint_progress() {
        let mut spring = SpringAnim::new(0.0, 400.0, 0.0, SNAP_SPRING_OMEGA);
        // 推进 ~100ms（ω=20 的 critically damped 约 63% 响应）
        for _ in 0..6 {
            spring.update(1.0 / 60.0);
        }
        let (pos, _vel, _done) = spring.update(0.0);
        // 半程位置应在 0~400 范围内，且已有显著进展
        assert!(pos > 50.0, "pos at ~100ms: {} (too slow)", pos);
        assert!(pos < 450.0, "pos at ~100ms: {} (overshoot too large)", pos);
    }

    /// spring 最终位置误差 < SNAP_THRESHOLD
    #[test]
    fn spring_final_accuracy() {
        let target = 256.5;
        let mut spring = SpringAnim::new(0.0, target, -500.0, SNAP_SPRING_OMEGA);
        let mut final_pos = 0.0;
        for _ in 0..600 {
            let (pos, _vel, done) = spring.update(1.0 / 60.0);
            final_pos = pos;
            if done {
                break;
            }
        }
        assert!(
            (final_pos - target).abs() < SNAP_THRESHOLD,
            "final: {}, target: {}, error: {}",
            final_pos,
            target,
            (final_pos - target).abs()
        );
    }

    /// 反向初速 spring 仍收敛（模拟 fling 反方向的 snap）
    #[test]
    fn spring_reverse_velocity() {
        let mut spring = SpringAnim::new(100.0, 300.0, -1000.0, SNAP_SPRING_OMEGA);
        for _ in 0..300 {
            let (pos, _vel, done) = spring.update(1.0 / 60.0);
            if done {
                assert!((pos - 300.0).abs() < SNAP_THRESHOLD);
                return;
            }
        }
        panic!("spring with reverse velocity did not converge");
    }

    // ══════════════════════════════════════════════
    // nearest_snap_point / nearest_snap_index
    // （对应 Chrome scroll_snap_data_unittest.cc — FindClosestSnapPosition）
    // ══════════════════════════════════════════════

    #[test]
    fn nearest_snap_empty() {
        assert_eq!(nearest_snap_point(&[], 100.0), None);
        assert_eq!(nearest_snap_index(&[], 100.0), None);
    }

    #[test]
    fn nearest_snap_single_point() {
        let points = vec![50.0];
        assert_eq!(nearest_snap_point(&points, 100.0), Some(50.0));
        assert_eq!(nearest_snap_index(&points, 100.0), Some(0));
    }

    #[test]
    fn nearest_snap_exact_match() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        assert_eq!(nearest_snap_point(&points, 200.0), Some(200.0));
        assert_eq!(nearest_snap_index(&points, 200.0), Some(2));
    }

    #[test]
    fn nearest_snap_between_points() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        // 130 更接近 100
        assert_eq!(nearest_snap_point(&points, 130.0), Some(100.0));
        assert_eq!(nearest_snap_index(&points, 130.0), Some(1));
        // 170 更接近 200
        assert_eq!(nearest_snap_point(&points, 170.0), Some(200.0));
        assert_eq!(nearest_snap_index(&points, 170.0), Some(2));
    }

    #[test]
    fn nearest_snap_past_last() {
        let points = vec![0.0, 100.0, 200.0];
        assert_eq!(nearest_snap_point(&points, 500.0), Some(200.0));
        assert_eq!(nearest_snap_index(&points, 500.0), Some(2));
    }

    #[test]
    fn nearest_snap_before_first() {
        let points = vec![100.0, 200.0, 300.0];
        assert_eq!(nearest_snap_point(&points, -50.0), Some(100.0));
        assert_eq!(nearest_snap_index(&points, -50.0), Some(0));
    }

    // ══════════════════════════════════════════════
    // directional_snap_point
    // ══════════════════════════════════════════════

    #[test]
    fn directional_snap_empty() {
        assert_eq!(directional_snap_point(&[], 100.0, 50.0, 1.0), None);
    }

    /// 正方向偏好：选择 current 之后的最近 snap 点
    #[test]
    fn directional_snap_forward() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        // current=50, predicted=150, direction=正 → 应选 200（100 在方向上但 200 更近 predicted）
        let result = directional_snap_point(&points, 150.0, 50.0, 1.0);
        // 方向过滤后 > 51.0: [100.0, 200.0, 300.0]，nearest to 150 = 200
        // 但 100 也是方向内候选且离 predicted 更近... 实际看算法
        // global nearest to 150 = 100 or 200 (等距时看排序)
        // directional nearest to 150 = 100 or 200
        // 因为 diff = |dir_nearest - predicted| - |global_nearest - predicted|
        // 如果 global=100: diff = |200-150| - |100-150| = 50-50 = 0 < 100 → 选 directional
        // 实际: nearest_snap_point([0,100,200,300], 150) → 100 (100和200等距，取先出现的)
        assert!(result == Some(100.0) || result == Some(200.0));
    }

    /// 负方向偏好
    #[test]
    fn directional_snap_backward() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        // current=250, predicted=150, direction=负 → < 249: [0, 100, 200]
        let result = directional_snap_point(&points, 150.0, 250.0, -1.0);
        // directional points < 249: [0, 100, 200]，nearest to 150 = 100 or 200
        assert!(result == Some(100.0) || result == Some(200.0));
    }

    /// 零方向（无偏好）— 返回全局最近
    #[test]
    fn directional_snap_no_direction() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        let result = directional_snap_point(&points, 190.0, 100.0, 0.0);
        assert_eq!(result, Some(200.0));
    }

    /// 方向上无候选点时回退到全局最近
    #[test]
    fn directional_snap_fallback_to_global() {
        let points = vec![0.0, 100.0, 200.0];
        // current=200, direction=正 → 方向过滤 > 201: 空
        let result = directional_snap_point(&points, 250.0, 200.0, 1.0);
        // 无方向候选 → 全局最近 to 250 = 200
        assert_eq!(result, Some(200.0));
    }

    /// 单个 snap 点
    #[test]
    fn directional_snap_single_point() {
        let points = vec![100.0];
        assert_eq!(directional_snap_point(&points, 50.0, 0.0, 1.0), Some(100.0));
        assert_eq!(directional_snap_point(&points, 150.0, 200.0, -1.0), Some(100.0));
    }

    // ── Chrome scroll_snap_data_unittest 补充移植 ──

    /// Chrome: MandatoryReturnsToCurrentIfNoValidAreaForward
    /// 方向无候选时回退到 current 最近 snap 点（保持不动），而非 predicted_end 最近
    #[test]
    fn directional_snap_returns_current_when_no_forward() {
        let points = vec![0.0, 100.0, 200.0, 300.0];
        // current=300（最后一个），direction=正 → 方向 > 301: 空
        let result = directional_snap_point(&points, 500.0, 300.0, 1.0);
        assert_eq!(result, Some(300.0));

        // current=0（第一个），direction=负 → 方向 < -1: 空
        let result2 = directional_snap_point(&points, -100.0, 0.0, -1.0);
        assert_eq!(result2, Some(0.0));
    }

    /// Chrome 差异测试：方向无候选时 predicted_end 附近的全局最近 ≠ current 最近
    #[test]
    fn directional_snap_returns_current_not_predicted() {
        let points = vec![0.0, 200.0, 400.0];
        // current=400, predicted=150, direction=正 → 方向 > 401: 空
        // 应回退到 nearest(points, current=400) = 400，而非 nearest(points, predicted=150) = 200
        let result = directional_snap_point(&points, 150.0, 400.0, 1.0);
        assert_eq!(
            result,
            Some(400.0),
            "should stay at current snap, not jump to predicted nearest"
        );
    }

    // ── Chrome snap_fling_curve_unittest 补充移植 ──

    /// Chrome: ReturnsZeroAfterFinished — 动画完成后继续 update 仍返回目标 + 零速度
    #[test]
    fn spring_returns_zero_after_finished() {
        let mut spring = SpringAnim::new(0.0, 300.0, 0.0, SNAP_SPRING_OMEGA);
        let mut done = false;
        for _ in 0..600 {
            let (_, _, d) = spring.update(1.0 / 60.0);
            if d {
                done = true;
                break;
            }
        }
        assert!(done, "spring should finish");
        // 完成后继续 update
        let (pos, vel, still_done) = spring.update(1.0 / 60.0);
        assert!(still_done, "should remain done");
        assert!((pos - 300.0).abs() < SNAP_THRESHOLD, "pos after done: {}", pos);
        assert!(vel.abs() < MIN_FLING_VELOCITY, "vel after done: {}", vel);
    }

    /// Chrome: CurveInitialization — 初始化后内部参数正确
    #[test]
    fn spring_initialization_values() {
        let start = 100.0;
        let target = 400.0;
        let v0 = 500.0;
        let spring = SpringAnim::new(start, target, v0, SNAP_SPRING_OMEGA);
        assert_eq!(spring.target, target);
        assert_eq!(spring.omega, SNAP_SPRING_OMEGA);
        assert_eq!(spring.a, start - target); // -300
        assert_eq!(spring.b, v0 + SNAP_SPRING_OMEGA * (start - target)); // 500 + 20*(-300) = -5500
        assert_eq!(spring.elapsed, 0.0);
    }
}
