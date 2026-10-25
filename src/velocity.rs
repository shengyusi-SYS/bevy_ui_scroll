// Chromium velocity-tracker attribution and BSD notice: see THIRD_PARTY_NOTICES.md
// and licenses/Chromium-BSD.txt. Copyright 2014 The Chromium Authors.
//! Chrome LSQ2 速度采样。
//!
//! 移植自 Chrome `VelocityTracker` — 使用二阶最小二乘 (LSQ2) 多项式拟合
//! 在时间窗口内的触摸样本，计算最新时刻的瞬时速度。
//! LSQ2_RESTRICTED 变体防止快速减速时的方向反转 (crbug.com/417855)。

use bevy::math::Vec2;

/// 速度采样窗口时长 (秒)
const FLING_VELOCITY_WINDOW: f32 = 0.1;

/// 速度采样缓冲区容量
const FLING_SAMPLE_CAP: usize = 20;

/// 惯性结束速度阈值 (px/s)
pub(super) const FLING_MIN_VELOCITY: f32 = 50.0;

/// B3: Fling 速度上限 (px/s)，对应 Android ViewConfiguration.getScaledMaximumFlingVelocity()
pub(super) const MAX_FLING_VELOCITY: f32 = 8000.0;

/// 2D 速度采样数据
#[derive(Clone, Copy)]
struct FlingTimeSample {
    time: f32,
    pos: Vec2,
}

/// 2D 速度采样器 — 使用 Chrome LSQ2 (Least Squares Quadratic) 速度估算。
///
/// Chrome VelocityTracker LSQ2 策略：对时间窗口内的样本进行二阶多项式拟合
///   p(t) = a + b*t + c*t²
/// 速度 = dp/dt|_{t=0} = b （以最新样本为 t=0 参考点）
///
/// LSQ2_RESTRICTED 变体：如果计算出的速度方向与首末差分方向相反，
/// 强制返回零速度（防止快速减速时的方向反转，参考 crbug.com/417855）
#[derive(Default)]
pub(super) struct FlingVelocitySampler {
    samples: Vec<FlingTimeSample>,
}

impl FlingVelocitySampler {
    pub(super) fn add(&mut self, time: f32, pos: Vec2) {
        self.samples.push(FlingTimeSample { time, pos });
        if self.samples.len() > FLING_SAMPLE_CAP {
            self.samples.remove(0);
        }
    }

    /// Chrome LSQ2 速度计算（带 RESTRICTED 防反向保护）。
    ///
    /// 1. 筛选时间窗口内的样本
    /// 2. 对每个轴做二阶最小二乘拟合 p(t) = a + b*t + c*t²
    /// 3. 速度 = b（在 t=0 即最新时刻的导数）
    /// 4. RESTRICTED：如果拟合速度方向与净位移方向相反，零化该轴（防快速减速后反向）
    pub(super) fn velocity(&self, now: f32) -> Vec2 {
        let cutoff = now - FLING_VELOCITY_WINDOW;
        let start = self.samples.iter().position(|s| s.time >= cutoff);
        let Some(start_idx) = start else {
            return Vec2::ZERO;
        };
        let window = &self.samples[start_idx..];
        if window.len() < 2 {
            return Vec2::ZERO;
        }

        let last = window.last().unwrap();
        let first = window.first().unwrap();
        let dt_total = last.time - first.time;
        if dt_total < 0.001 {
            return Vec2::ZERO;
        }

        // 如果样本不足 3 个，回退到线性（LSQ1）
        if window.len() < 3 {
            return (last.pos - first.pos) / dt_total;
        }

        // LSQ2: 二阶多项式最小二乘拟合
        // 以最新样本为时间原点（t_i = sample_time - last_time，全部 ≤ 0）
        let vx = Self::lsq2_velocity(window, last.time, |s| s.pos.x);
        let vy = Self::lsq2_velocity(window, last.time, |s| s.pos.y);

        // LSQ2_RESTRICTED：净位移方向与拟合速度方向不同时，零化
        let net = last.pos - first.pos;
        let vx = if net.x * vx < 0.0 { 0.0 } else { vx };
        let vy = if net.y * vy < 0.0 { 0.0 } else { vy };

        Vec2::new(vx, vy)
    }

    /// 对单轴样本做 LSQ2 拟合，返回 t=0 处的速度（即最新时刻的速度）。
    ///
    /// 拟合: p(t) = a + b*t + c*t²
    /// 正规方程: [Σ1, Σt, Σt²; Σt, Σt², Σt³; Σt², Σt³, Σt⁴] * [a,b,c]ᵀ = [Σp, Σtp, Σt²p]ᵀ
    /// 速度 = dp/dt|_{t=0} = b
    fn lsq2_velocity(window: &[FlingTimeSample], t_ref: f32, axis: impl Fn(&FlingTimeSample) -> f32) -> f32 {
        let n = window.len() as f32;
        let mut s1 = 0.0_f32;
        let mut s2 = 0.0_f32;
        let mut s3 = 0.0_f32;
        let mut s4 = 0.0_f32;
        let mut sp = 0.0_f32;
        let mut stp = 0.0_f32;
        let mut st2p = 0.0_f32;

        for sample in window {
            let t = sample.time - t_ref; // ≤ 0
            let p = axis(sample);
            let t2 = t * t;
            s1 += t;
            s2 += t2;
            s3 += t2 * t;
            s4 += t2 * t2;
            sp += p;
            stp += t * p;
            st2p += t2 * p;
        }

        // 正规方程矩阵:
        // | n   s1  s2 | | a |   | sp   |
        // | s1  s2  s3 | | b | = | stp  |
        // | s2  s3  s4 | | c |   | st2p |
        //
        // 用 Cramer 法则求 b

        let det = n * (s2 * s4 - s3 * s3) - s1 * (s1 * s4 - s3 * s2) + s2 * (s1 * s3 - s2 * s2);

        if det.abs() < 1e-12 {
            // 矩阵奇异，回退到线性
            let first = window.first().unwrap();
            let last = window.last().unwrap();
            let dt = last.time - first.time;
            if dt.abs() < 0.001 {
                return 0.0;
            }
            return (axis(last) - axis(first)) / dt;
        }

        // b 的 Cramer 列替换（替换第二列）
        let det_b = n * (stp * s4 - s3 * st2p) - sp * (s1 * s4 - s3 * s2) + s2 * (s1 * st2p - stp * s2);

        det_b / det
    }
}

/// 将 fling 速度按轴独立钳位到 MAX_FLING_VELOCITY。
///
/// Chrome VelocityTracker: `ComputeCurrentVelocity(1000, maxVelocity)` 对每个轴独立 clamp，
/// 不是按向量长度。参考 velocity_tracker_unittest.cc MaxVelocity 测试：
///   velocity (-50000, 50000), max=100 → (-100, 100)
pub(super) fn clamp_fling_velocity(velocity: Vec2) -> Vec2 {
    Vec2::new(
        velocity.x.clamp(-MAX_FLING_VELOCITY, MAX_FLING_VELOCITY),
        velocity.y.clamp(-MAX_FLING_VELOCITY, MAX_FLING_VELOCITY),
    )
}

/// 单轴速度采样器 — 与 FlingVelocitySampler 共享相同 LSQ2 算法。
///
/// 供 snap 模块使用（snap 模块只需要单轴速度）。
#[derive(Default)]
pub(super) struct SingleAxisVelocitySampler {
    samples: Vec<SingleAxisSample>,
}

#[derive(Clone, Copy)]
struct SingleAxisSample {
    time: f32,
    position: f32,
}

/// snap 模块使用的速度采样窗口常量
const SNAP_VELOCITY_WINDOW_SECS: f32 = 0.1;
/// snap 模块使用的速度采样缓冲区容量
const SNAP_VELOCITY_SAMPLE_CAP: usize = 20;

impl SingleAxisVelocitySampler {
    pub(super) fn reset(&mut self) {
        self.samples.clear();
    }

    pub(super) fn add_sample(&mut self, time: f32, position: f32) {
        self.samples.push(SingleAxisSample { time, position });
        if self.samples.len() > SNAP_VELOCITY_SAMPLE_CAP {
            self.samples.remove(0);
        }
    }

    /// Chrome LSQ2 速度计算（带 RESTRICTED 防反向保护），单轴版本。
    pub(super) fn compute_velocity(&self, current_time: f32) -> f32 {
        let cutoff = current_time - SNAP_VELOCITY_WINDOW_SECS;
        let window_start = self.samples.iter().position(|s| s.time >= cutoff);
        let Some(start_idx) = window_start else {
            return 0.0;
        };
        let window = &self.samples[start_idx..];
        if window.len() < 2 {
            return 0.0;
        }
        let first = window.first().unwrap();
        let last = window.last().unwrap();
        let dt_total = last.time - first.time;
        if dt_total < 0.001 {
            return 0.0;
        }

        // 不足 3 个样本回退到线性
        if window.len() < 3 {
            return (last.position - first.position) / dt_total;
        }

        // LSQ2 拟合
        let t_ref = last.time;
        let n = window.len() as f32;
        let mut s1 = 0.0_f32;
        let mut s2 = 0.0_f32;
        let mut s3 = 0.0_f32;
        let mut s4 = 0.0_f32;
        let mut sp = 0.0_f32;
        let mut stp = 0.0_f32;
        let mut st2p = 0.0_f32;

        for sample in window {
            let t = sample.time - t_ref;
            let p = sample.position;
            let t2 = t * t;
            s1 += t;
            s2 += t2;
            s3 += t2 * t;
            s4 += t2 * t2;
            sp += p;
            stp += t * p;
            st2p += t2 * p;
        }

        let det = n * (s2 * s4 - s3 * s3) - s1 * (s1 * s4 - s3 * s2) + s2 * (s1 * s3 - s2 * s2);

        let vel = if det.abs() < 1e-12 {
            (last.position - first.position) / dt_total
        } else {
            let det_b = n * (stp * s4 - s3 * st2p) - sp * (s1 * s4 - s3 * s2) + s2 * (s1 * st2p - stp * s2);
            det_b / det
        };

        // LSQ2_RESTRICTED: 防反向
        let net = last.position - first.position;
        if net * vel < 0.0 { 0.0 } else { vel }
    }
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    /// 零样本返回零速度
    #[test]
    fn fling_sampler_no_samples() {
        let sampler = FlingVelocitySampler::default();
        assert_eq!(sampler.velocity(1.0), Vec2::ZERO);
    }

    /// 单样本返回零速度（无法计算差分）
    #[test]
    fn fling_sampler_single_sample() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(1.0, Vec2::new(100.0, 200.0));
        assert_eq!(sampler.velocity(1.0), Vec2::ZERO);
    }

    /// 线性运动产生正确速度
    #[test]
    fn fling_sampler_linear_velocity() {
        let mut sampler = FlingVelocitySampler::default();
        // 100 px/s 水平，200 px/s 垂直
        sampler.add(1.00, Vec2::new(0.0, 0.0));
        sampler.add(1.02, Vec2::new(2.0, 4.0));
        sampler.add(1.04, Vec2::new(4.0, 8.0));
        sampler.add(1.06, Vec2::new(6.0, 12.0));
        sampler.add(1.08, Vec2::new(8.0, 16.0));
        let v = sampler.velocity(1.08);
        assert!((v.x - 100.0).abs() < 1.0, "x velocity: {}", v.x);
        assert!((v.y - 200.0).abs() < 1.0, "y velocity: {}", v.y);
    }

    /// 窗口期外旧样本被丢弃（参考 Chrome velocity_tracker_unittest Basic）
    #[test]
    fn fling_sampler_old_samples_discarded() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(0.0, Vec2::new(0.0, 0.0));
        sampler.add(0.05, Vec2::new(50.0, 0.0));
        sampler.add(0.90, Vec2::new(200.0, 0.0));
        sampler.add(0.95, Vec2::new(205.0, 0.0));
        sampler.add(0.99, Vec2::new(209.0, 0.0));
        let v = sampler.velocity(0.99);
        assert!((v.x - 100.0).abs() < 1.0, "velocity: {}", v.x);
    }

    /// 同时间戳样本不导致除零
    #[test]
    fn fling_sampler_same_timestamp() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(1.0, Vec2::new(0.0, 0.0));
        sampler.add(1.0, Vec2::new(10.0, 0.0));
        let v = sampler.velocity(1.0);
        assert_eq!(v, Vec2::ZERO);
    }

    /// 缓冲区容量上限测试
    #[test]
    fn fling_sampler_capacity_overflow() {
        let mut sampler = FlingVelocitySampler::default();
        for i in 0..(FLING_SAMPLE_CAP + 5) {
            sampler.add(i as f32 * 0.01, Vec2::new(i as f32 * 10.0, 0.0));
        }
        assert_eq!(sampler.samples.len(), FLING_SAMPLE_CAP);
    }

    /// 窗口期内只有一个样本时返回零
    #[test]
    fn fling_sampler_window_single_sample() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(0.0, Vec2::new(0.0, 0.0));
        sampler.add(1.0, Vec2::new(500.0, 0.0));
        let v = sampler.velocity(1.0);
        assert_eq!(v, Vec2::ZERO);
    }

    /// 负方向速度
    #[test]
    fn fling_sampler_negative_velocity() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(1.00, Vec2::new(100.0, 0.0));
        sampler.add(1.05, Vec2::new(50.0, 0.0));
        sampler.add(1.09, Vec2::new(10.0, 0.0));
        let v = sampler.velocity(1.09);
        assert!((v.x - (-1000.0)).abs() < 1.0, "velocity: {}", v.x);
    }

    /// Chrome velocity_tracker_unittest: MaxVelocity
    #[test]
    fn fling_sampler_max_velocity() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(1.00, Vec2::new(0.0, 0.0));
        sampler.add(1.01, Vec2::new(200.0, 0.0));
        let raw = sampler.velocity(1.01);
        assert!(raw.x > MAX_FLING_VELOCITY);
        let clamped = clamp_fling_velocity(raw);
        assert!(clamped.x <= MAX_FLING_VELOCITY + 0.1, "clamped x {} > MAX", clamped.x);
        assert!(clamped.x > 0.0);
    }

    /// MaxVelocity 2D — Chrome 按轴独立钳位（不是向量长度）
    #[test]
    fn fling_sampler_max_velocity_2d() {
        let v = Vec2::new(-10000.0, 10000.0);
        let c = clamp_fling_velocity(v);
        assert_eq!(c.x, -MAX_FLING_VELOCITY);
        assert_eq!(c.y, MAX_FLING_VELOCITY);
    }

    /// clamp_fling_velocity: 未超限时按轴保持原值
    #[test]
    fn fling_sampler_clamp_below_limit() {
        let v = Vec2::new(3000.0, 4000.0);
        let c = clamp_fling_velocity(v);
        assert!((c - v).length() < 0.01, "should not clamp: {:?}", c);
    }

    /// Chrome velocity_tracker_unittest: VaryingVelocity
    #[test]
    fn fling_sampler_varying_velocity() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(1.00, Vec2::new(0.0, 0.0));
        sampler.add(1.01, Vec2::new(5.5, 0.0));
        sampler.add(1.03, Vec2::new(19.5, 0.0));
        sampler.add(1.04, Vec2::new(28.0, 0.0));
        sampler.add(1.08, Vec2::new(72.0, 0.0));
        let v = sampler.velocity(1.08);
        assert!(v.x > 800.0, "LSQ2 velocity should reflect acceleration, got: {}", v.x);
        assert!(v.x < 1500.0, "LSQ2 velocity too high: {}", v.x);
    }

    /// Chrome velocity_tracker_unittest: DelayedActionUp
    #[test]
    fn fling_sampler_delayed_action_up() {
        let mut sampler = FlingVelocitySampler::default();
        sampler.add(0.00, Vec2::new(0.0, 0.0));
        sampler.add(0.02, Vec2::new(20.0, 0.0));
        sampler.add(0.04, Vec2::new(40.0, 0.0));
        sampler.add(0.06, Vec2::new(60.0, 0.0));
        sampler.add(0.08, Vec2::new(80.0, 0.0));
        let v = sampler.velocity(0.28);
        assert!(v.length() < 1.0, "delayed velocity should be ~0, got: {}", v.length());
    }

    /// Chrome velocity_tracker_unittest: NoDirectionReversal
    #[test]
    fn fling_sampler_no_direction_reversal() {
        let mut sampler = FlingVelocitySampler::default();
        let t0 = 1.0_f32;
        let dt = 0.001;
        let mut y = 0.0_f32;
        sampler.add(t0, Vec2::new(0.0, y));
        for i in 1..60 {
            if i < 50 {
                y += 10.0;
            }
            sampler.add(t0 + dt * i as f32, Vec2::new(0.0, y));
        }
        let now = t0 + dt * 59.0;
        let v = sampler.velocity(now);
        assert!(
            v.y >= 0.0,
            "NoDirectionReversal: y velocity should not be negative (got {})",
            v.y
        );
    }

    // ── SingleAxisVelocitySampler ──

    /// 单轴采样器与 2D 采样器在单轴数据上产生相同结果
    #[test]
    fn single_axis_matches_2d() {
        let mut sa = SingleAxisVelocitySampler::default();
        let mut fa = FlingVelocitySampler::default();

        sa.add_sample(1.00, 0.0);
        sa.add_sample(1.02, 2.0);
        sa.add_sample(1.04, 4.0);
        sa.add_sample(1.06, 6.0);
        sa.add_sample(1.08, 8.0);

        fa.add(1.00, Vec2::new(0.0, 0.0));
        fa.add(1.02, Vec2::new(2.0, 0.0));
        fa.add(1.04, Vec2::new(4.0, 0.0));
        fa.add(1.06, Vec2::new(6.0, 0.0));
        fa.add(1.08, Vec2::new(8.0, 0.0));

        let sv = sa.compute_velocity(1.08);
        let fv = fa.velocity(1.08).x;
        assert!((sv - fv).abs() < 0.5, "single={sv}, 2d={fv}");
    }
}
