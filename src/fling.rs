// Portions adapted from Android OverScroller. Copyright (C) 2010 The Android Open Source Project.
// Licensed under Apache-2.0; see THIRD_PARTY_NOTICES.md and LICENSE-APACHE.
//! Chrome SplineOverScroller 惯性滑行。
//!
//! 对标 Android `SplineOverScroller` (frameworks/base/core/java/android/widget/OverScroller.java)：
//! - S 型参数化三次样条曲线（编译期预计算查找表）
//! - 距离 ∝ velocity^1.736（幂律，非指数衰减）
//! - 时长 ∝ velocity^0.736
//! - 各轴独立计算距离/时长

use bevy::{
    ecs::{
        entity::Entity,
        message::MessageWriter,
        system::{Query, Res, ResMut},
    },
    math::Vec2,
    time::Time,
    ui::{ComputedNode, Node, OverflowAxis, ScrollPosition},
    window::RequestRedraw,
};

use super::overscroll::{OverscrollBounceState, OverscrollEntry, OverscrollSpring};
use super::snap::ScrollSnapState;

// ── Chrome SplineOverScroller 曲线常量 ──

/// Android DECELERATION_RATE = ln(0.78) / ln(0.9)
const DECELERATION_RATE: f64 = 2.358_201_589;
/// S 型样条拐点
const INFLEXION: f64 = 0.35;
/// S 型样条起始张力
const START_TENSION: f64 = 0.5;
/// S 型样条结束张力
const END_TENSION: f64 = 1.0;
/// 样条查找表大小
const NB_SAMPLES: usize = 100;

/// fling 摩擦系数 — Android ViewConfiguration.getScrollFriction()
const FLING_FRICTION: f64 = 0.015;
/// 参考设备密度 (dp/inch) — Android mdpi 基线
const REFERENCE_PPI: f64 = 160.0;

/// Snap 容器专用的指数衰减系数（保留旧模型，snap 不使用 spline）
pub(super) const FLING_DECAY: f32 = 4.0;

/// Chrome SplineOverScroller 查找表（编译期预计算）。
struct SplineLookup {
    position: [f32; NB_SAMPLES + 1],
    time: [f32; NB_SAMPLES + 1],
}

/// 编译期生成 spline 查找表
const fn build_spline_lookup() -> SplineLookup {
    let p1 = START_TENSION * INFLEXION;
    let p2 = 1.0 - END_TENSION * (1.0 - INFLEXION);

    let mut position = [0.0f32; NB_SAMPLES + 1];
    let mut time = [0.0f32; NB_SAMPLES + 1];

    let mut i = 0;
    while i < NB_SAMPLES + 1 {
        let alpha = i as f64 / NB_SAMPLES as f64;

        // 二分法求解三次贝塞尔 t 值
        let mut lo = 0.0_f64;
        let mut hi = 1.0_f64;
        let mut t;
        let mut j = 0;
        loop {
            t = (lo + hi) / 2.0;
            let coef = 3.0 * t * (1.0 - t);
            let tx = coef * ((1.0 - t) * p1 + t * p2) + t * t * t;
            if tx > alpha + 1e-8 {
                hi = t;
            } else if tx < alpha - 1e-8 {
                lo = t;
            } else {
                break;
            }
            j += 1;
            if j > 100 {
                break;
            }
        }

        let coef = 3.0 * t * (1.0 - t);
        let y = coef * ((1.0 - t) * START_TENSION + t) + t * t * t;

        position[i] = y as f32;
        time[i] = alpha as f32;

        i += 1;
    }

    SplineLookup { position, time }
}

/// 静态查找表
const SPLINE: SplineLookup = build_spline_lookup();

/// Android SplineOverScroller 的物理系数
const fn physical_coeff(ppi: f64) -> f64 {
    9.806_65 * 39.37 * ppi * 0.84
}

/// 使用参考 PPI 的物理系数
const PHYSICAL_COEFF: f64 = physical_coeff(REFERENCE_PPI);

/// 计算 spline fling 总距离 (px)
pub(super) fn spline_fling_distance(velocity: f64) -> f64 {
    let l = (INFLEXION * velocity.abs() / (FLING_FRICTION * PHYSICAL_COEFF)).ln();
    FLING_FRICTION * PHYSICAL_COEFF * (DECELERATION_RATE / (DECELERATION_RATE - 1.0) * l).exp()
}

/// 计算 spline fling 总时长 (秒)
pub(super) fn spline_fling_duration(velocity: f64) -> f64 {
    let l = (INFLEXION * velocity.abs() / (FLING_FRICTION * PHYSICAL_COEFF)).ln();
    (l / (DECELERATION_RATE - 1.0)).exp()
}

/// 给定 time_fraction (0~1)，通过查找表返回 position_fraction (0~1)
fn spline_sample(time_fraction: f32) -> (f32, f32) {
    let index = (NB_SAMPLES as f32 * time_fraction) as usize;
    if index >= NB_SAMPLES {
        return (1.0, 0.0);
    }

    let t_inf = SPLINE.time[index];
    let t_sup = SPLINE.time[index + 1];
    let d_inf = SPLINE.position[index];
    let d_sup = SPLINE.position[index + 1];

    let time_diff = t_sup - t_inf;
    if time_diff > 0.0 {
        let slope = (d_sup - d_inf) / time_diff;
        (d_inf + (time_fraction - t_inf) * slope, slope)
    } else {
        (d_inf, 0.0)
    }
}

#[cfg(test)]
fn spline_position(time_fraction: f32) -> f32 {
    spline_sample(time_fraction).0
}

// ── Fling 状态 ──

/// 活跃的 fling 条目（Chrome SplineOverScroller 模型）
pub(super) struct FlingEntry {
    /// Scroll Session 锁定的滚动目标
    pub(super) target: Entity,
    /// 各轴总 fling 距离 (px)（带方向符号）
    pub(super) total_distance: Vec2,
    /// 各轴 fling 总时长 (秒)
    pub(super) duration: Vec2,
    /// 已经过时间 (秒)
    pub(super) elapsed: f32,
    /// 上一帧的各轴位置分数（0~1）
    pub(super) prev_position_fraction: Vec2,
}

/// 全局 fling 状态（Resource，多个 system 共享）
#[derive(Default, bevy::ecs::resource::Resource)]
pub(super) struct FlingState {
    pub(super) entries: Vec<FlingEntry>,
}

// ── Fling 系统 ──

/// D1: 触摸惯性滑行系统。
///
/// 使用 Chrome SplineOverScroller 曲线（S 型参数化三次样条）。
/// 各轴独立计算距离/时长，通过编译期生成的查找表做 time→position 映射。
pub(super) fn touch_fling_system(
    mut fling_state: ResMut<FlingState>,
    time: Res<Time>,
    snap_q: Query<(), bevy::ecs::query::With<ScrollSnapState>>,
    mut scroll_q: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
    mut bounce_state: ResMut<OverscrollBounceState>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    let mut moved = false;
    fling_state.entries.retain_mut(|entry| {
        // Snap owns this trajectory; do not retain a second idle animation.
        if snap_q.contains(entry.target) {
            return false;
        }
        entry.elapsed += dt;

        // 各轴的 time_fraction 和 position_fraction（独立时长）
        let tf_x = if entry.duration.x > 0.0 {
            (entry.elapsed / entry.duration.x).min(1.0)
        } else {
            1.0
        };
        let tf_y = if entry.duration.y > 0.0 {
            (entry.elapsed / entry.duration.y).min(1.0)
        } else {
            1.0
        };

        let (pf_x, slope_x) = spline_sample(tf_x);
        let (pf_y, slope_y) = spline_sample(tf_y);

        // 本帧 delta = (当前 position_fraction - 上帧 position_fraction) × total_distance
        let delta = Vec2::new(
            (pf_x - entry.prev_position_fraction.x) * entry.total_distance.x,
            (pf_y - entry.prev_position_fraction.y) * entry.total_distance.y,
        );

        entry.prev_position_fraction = Vec2::new(pf_x, pf_y);

        // Scroll Session: 发送给锁定的唯一目标
        // §2: 显式排除 snap 容器 — snap 容器有独立的惯性/spring 系统
        let target = entry.target;
        if let Ok((mut scroll_pos, node, computed)) = scroll_q.get_mut(target) {
            let previous = scroll_pos.0;
            let max_offset = crate::max_scroll_offset(computed);
            let mut hit_boundary = false;

            if node.overflow.x == OverflowAxis::Scroll && delta.x != 0.0 {
                let max_x = max_offset.x.max(0.0);
                let new_x = scroll_pos.x + delta.x;
                if new_x < 0.0 || new_x > max_x {
                    let clamped = new_x.clamp(0.0, max_x);
                    let remaining_vel = if entry.duration.x > 0.0 {
                        slope_x * entry.total_distance.x / entry.duration.x
                    } else {
                        0.0
                    };
                    let target_x = if new_x < 0.0 { 0.0 } else { max_x };
                    scroll_pos.x = clamped;
                    let bounce_entry = bounce_state.entries.entry(target).or_insert_with(|| OverscrollEntry {
                        spring_x: None,
                        spring_y: None,
                        was_touching: false,
                    });
                    bounce_entry.spring_x = Some(OverscrollSpring::new(clamped, target_x, remaining_vel * 0.3));
                    hit_boundary = true;
                } else {
                    scroll_pos.x = new_x;
                }
            }

            if node.overflow.y == OverflowAxis::Scroll && delta.y != 0.0 {
                let max_y = max_offset.y.max(0.0);
                let new_y = scroll_pos.y + delta.y;
                if new_y < 0.0 || new_y > max_y {
                    let clamped = new_y.clamp(0.0, max_y);
                    let remaining_vel = if entry.duration.y > 0.0 {
                        slope_y * entry.total_distance.y / entry.duration.y
                    } else {
                        0.0
                    };
                    let target_y = if new_y < 0.0 { 0.0 } else { max_y };
                    scroll_pos.y = clamped;
                    let bounce_entry = bounce_state.entries.entry(target).or_insert_with(|| OverscrollEntry {
                        spring_x: None,
                        spring_y: None,
                        was_touching: false,
                    });
                    bounce_entry.spring_y = Some(OverscrollSpring::new(clamped, target_y, remaining_vel * 0.3));
                    hit_boundary = true;
                } else {
                    scroll_pos.y = new_y;
                }
            }

            moved |= scroll_pos.0 != previous;
            if hit_boundary {
                bevy::log::trace!("[fling] Boundary hit, starting bounce on {target:?}");
                return false;
            }

            bevy::log::trace!(
                "[fling] Frame: elapsed={:.3}s delta={delta:?} target={target:?} (spline)",
                entry.elapsed
            );
        } else {
            return false;
        }

        // Apply the terminal sample before ending, even for a large last dt.
        tf_x < 1.0 || tf_y < 1.0
    });

    if moved || !fling_state.entries.is_empty() {
        redraw.write(RequestRedraw);
    }
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    /// Snap 容器专用指数衰减 fling 总距离 = v0 / decay
    #[test]
    fn fling_curve_total_distance() {
        let v0: f32 = 2000.0;
        let total = v0 / FLING_DECAY;
        assert!((total - 500.0).abs() < 0.01);
        let t_large = 5.0;
        let fraction = 1.0 - (-FLING_DECAY * t_large).exp();
        assert!((fraction - 1.0).abs() < 0.001);
    }

    /// 指数衰减中间时刻 t=τ 时 fraction ≈ 0.632 (1 - 1/e)
    #[test]
    fn fling_curve_midpoint_fraction() {
        let t = 1.0 / FLING_DECAY;
        let fraction = 1.0 - (-FLING_DECAY * t).exp();
        assert!((fraction - 0.6321).abs() < 0.01, "fraction at τ: {}", fraction);
    }

    /// 指数衰减 duration 计算正确
    #[test]
    fn fling_curve_duration() {
        use super::super::velocity::FLING_MIN_VELOCITY;
        let v0 = 2000.0_f32;
        let duration = (v0 / FLING_MIN_VELOCITY).ln() / FLING_DECAY;
        let v_end = v0 * (-FLING_DECAY * duration).exp();
        assert!((v_end - FLING_MIN_VELOCITY).abs() < 0.1, "v at duration: {}", v_end);
    }

    /// spline_position(0) ≈ 0
    #[test]
    fn spline_position_at_zero() {
        let p = spline_position(0.0);
        assert!(p.abs() < 0.01, "spline_position(0) = {p}");
    }

    /// spline_position(1) ≈ 1
    #[test]
    fn spline_position_at_one() {
        let p = spline_position(1.0);
        assert!((p - 1.0).abs() < 0.01, "spline_position(1) = {p}");
    }

    /// spline_position 单调递增
    #[test]
    fn spline_position_monotonic() {
        let mut prev = 0.0_f32;
        for i in 1..=100 {
            let t = i as f32 / 100.0;
            let p = spline_position(t);
            assert!(p >= prev, "non-monotonic at t={t}: {p} < {prev}");
            prev = p;
        }
    }

    /// spline_fling_distance 正值输入返回正距离
    #[test]
    fn spline_fling_distance_positive() {
        let d = spline_fling_distance(2000.0);
        assert!(d > 0.0, "distance should be positive, got {d}");
        assert!(d < 10000.0, "distance should be reasonable, got {d}");
    }

    /// spline_fling_duration 正值输入返回正时长
    #[test]
    fn spline_fling_duration_positive() {
        let t = spline_fling_duration(2000.0);
        assert!(t > 0.0, "duration should be positive, got {t}");
        assert!(t < 10.0, "duration should be reasonable, got {t}");
    }
}
