use crate::{EnhancedScroll, ScrollInput, ScrollInputKind};
use bevy::{prelude::*, window::RequestRedraw};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum ScrollBehavior {
    #[default]
    Instant,
    Smooth,
}

/// Logical-pixel offset request. Replaces previous motion on this viewport only.
/// Waits for nonzero layout; real input or `StopScroll` cancels the wait/animation.
#[derive(EntityEvent)]
pub struct ScrollToOffset {
    pub entity: Entity,
    pub offset: Vec2,
    pub behavior: ScrollBehavior,
}

#[derive(Component)]
pub(crate) struct OffsetAnimation {
    target: Vec2,
    start: Option<Vec2>,
    elapsed: f32,
    behavior: ScrollBehavior,
}

pub(crate) fn stop_motion(world: &mut World, entity: Entity) {
    if let Some(mut fling) = world.get_resource_mut::<crate::fling::FlingState>() {
        fling.entries.retain(|entry| entry.target != entity);
    }
    if let Some(mut bounce) = world.get_resource_mut::<crate::overscroll::OverscrollBounceState>() {
        bounce.entries.remove(&entity);
    }
    if let Ok(mut target) = world.get_entity_mut(entity) {
        target.remove::<(OffsetAnimation, crate::snap::PendingSnapRequest)>();
    }
    if let Some(mut smooth) = world.get_mut::<crate::event::SmoothScrollState>(entity) {
        *smooth = default();
    }
    if let Some(mut snap) = world.get_mut::<crate::snap::ScrollSnapState>(entity) {
        *snap = default();
        snap.suspended = true;
    }
}

pub(crate) fn on_scroll_to_offset(event: On<ScrollToOffset>, mut commands: Commands) {
    if !event.offset.is_finite() {
        return;
    }
    let (entity, offset, behavior) = (event.entity, event.offset, event.behavior);
    commands.queue(move |world: &mut World| {
        if world.get::<EnhancedScroll>(entity).is_none() {
            return;
        }
        stop_motion(world, entity);
        if behavior == ScrollBehavior::Instant {
            let ready = world
                .get::<ComputedNode>(entity)
                .filter(|node| node.size().min_element() > 0.0)
                .map(|node| offset.clamp(Vec2::ZERO, crate::max_scroll_offset(node)));
            if let Some(target) = ready {
                let axes = world.get::<Node>(entity).map(|node| {
                    (
                        node.overflow.x == OverflowAxis::Scroll,
                        node.overflow.y == OverflowAxis::Scroll,
                    )
                });
                if let Some((x, y)) = axes
                    && let Some(mut position) = world.get_mut::<ScrollPosition>(entity)
                {
                    if x {
                        position.x = target.x;
                    }
                    if y {
                        position.y = target.y;
                    }
                    world.write_message(RequestRedraw);
                    return;
                }
            }
        }
        world.entity_mut(entity).insert(OffsetAnimation {
            target: offset,
            start: None,
            elapsed: 0.0,
            behavior,
        });
        world.write_message(RequestRedraw);
    });
}

pub(crate) fn on_scroll_input(event: On<ScrollInput>, mut commands: Commands) {
    let (entity, kind) = (event.entity, event.kind);
    commands.queue(move |world: &mut World| {
        if let Ok(mut target) = world.get_entity_mut(entity) {
            target.remove::<(OffsetAnimation, crate::snap::PendingSnapRequest)>();
        }
        if kind == ScrollInputKind::Wheel {
            if let Some(mut fling) = world.get_resource_mut::<crate::fling::FlingState>() {
                fling.entries.retain(|entry| entry.target != entity);
            }
            if let Some(mut bounce) = world.get_resource_mut::<crate::overscroll::OverscrollBounceState>() {
                bounce.entries.remove(&entity);
            }
        }
    });
}

pub(crate) fn animate_offsets(
    mut targets: Query<(Entity, &Node, &ComputedNode, &mut ScrollPosition, &mut OffsetAnimation)>,
    time: Res<Time>,
    mut commands: Commands,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    for (entity, node, computed, mut position, mut animation) in &mut targets {
        if computed.size().x <= 0.0 || computed.size().y <= 0.0 {
            continue;
        }
        let max = crate::max_scroll_offset(computed);
        let target = animation.target.clamp(Vec2::ZERO, max);
        let start = *animation.start.get_or_insert(position.0);
        animation.elapsed += time.delta_secs();
        let t = if animation.behavior == ScrollBehavior::Instant {
            1.0
        } else {
            (animation.elapsed / 0.25).min(1.0)
        };
        let next = start.lerp(target, 1.0 - (1.0 - t).powi(3));
        if node.overflow.x == OverflowAxis::Scroll {
            position.x = next.x;
        }
        if node.overflow.y == OverflowAxis::Scroll {
            position.y = next.y;
        }
        if t >= 1.0 {
            commands.entity(entity).remove::<OffsetAnimation>();
        }
        redraw.write(RequestRedraw);
    }
}

// Reactive windows need one follow-up when an initially unlaid-out request becomes ready.
pub(crate) fn wake_ready_offsets(
    targets: Query<(&ComputedNode, &OffsetAnimation)>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    if targets
        .iter()
        .any(|(computed, animation)| animation.start.is_none() && computed.size().min_element() > 0.0)
    {
        redraw.write(RequestRedraw);
    }
}
