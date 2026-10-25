//! Private motion math shared by wheel smoothing, snap and boundary return.

/// Displacement and velocity of a critically damped spring at elapsed `time`.
/// `a = start - target`, `b = initial_velocity + omega * a`.
pub(crate) fn spring_sample(a: f32, b: f32, omega: f32, time: f32) -> (f32, f32) {
    let decay = (-omega * time).exp();
    let displacement = (a + b * time) * decay;
    let velocity = (b - omega * (a + b * time)) * decay;
    (displacement, velocity)
}
