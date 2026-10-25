use bevy::{
    input::{keyboard::KeyCode, mouse::MouseScrollUnit},
    picking::events::{Pointer, Press, Scroll as PointerScroll},
    prelude::*,
    ui_widgets::{ScrollArea, Scrollbar, ScrollbarThumb},
};

use crate::{EnhancedScroll, ScrollSource, StopScroll};

/// User intent at an actual viewport, including input at its boundary.
/// Consumers can cancel pending restoration/navigation without inspecting offsets.
#[derive(EntityEvent)]
pub struct ScrollInput {
    pub entity: Entity,
    pub kind: ScrollInputKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollInputKind {
    Wheel,
    Touch,
    Scrollbar,
}

pub(crate) fn on_pointer_wheel(
    mut event: On<Pointer<PointerScroll>>,
    enhanced: Query<(), (With<EnhancedScroll>, Without<ScrollArea>)>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut commands: Commands,
) {
    if !enhanced.contains(event.entity) {
        return;
    }
    // Native picking dispatches once to the hit branch. Stop its native bubbling;
    // our normalized event handles the remaining enhanced ancestors.
    event.propagate(false);
    let mut delta = -Vec2::new(event.x, event.y);
    if event.unit == MouseScrollUnit::Line {
        delta *= 60.0;
    }
    if keys.is_some_and(|keys| keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])) {
        std::mem::swap(&mut delta.x, &mut delta.y);
    }
    commands.trigger(crate::Scroll {
        entity: event.entity,
        delta,
        original_delta: delta,
        source: ScrollSource::Wheel,
        unit: event.unit,
    });
}

pub(crate) fn on_scrollbar_press(
    event: On<Pointer<Press>>,
    bars: Query<&Scrollbar>,
    thumbs: Query<&ChildOf, With<ScrollbarThumb>>,
    mut commands: Commands,
) {
    // Native thumbs stop Press propagation; resolve their track at the thumb itself.
    let track = thumbs.get(event.entity).map_or(event.entity, ChildOf::parent);
    if let Ok(bar) = bars.get(track) {
        commands.trigger(StopScroll { entity: bar.target });
        commands.trigger(ScrollInput {
            entity: bar.target,
            kind: ScrollInputKind::Scrollbar,
        });
    }
}
