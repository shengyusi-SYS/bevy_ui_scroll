use std::collections::HashMap;

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        lifecycle::RemovedComponents,
        message::MessageReader,
        observer::On,
        query::With,
        reflect::ReflectComponent,
        system::{Query, ResMut},
    },
    input::touch::{TouchInput, TouchPhase},
    picking::{
        events::{Cancel, DragEnd, Pointer, Press, Release},
        pointer::PointerId,
    },
    prelude::Resource,
    reflect::{Reflect, prelude::ReflectDefault},
};

#[derive(Component, Clone, Copy, Default, Reflect)]
#[reflect(Component, Clone, Default)]
pub struct ScrollGestureCapture;

#[derive(Resource, Default)]
pub(crate) struct TouchGestureCaptureState {
    owners_by_touch: HashMap<u64, Entity>,
}

#[derive(Resource, Default)]
pub(crate) struct PendingTouchGestureCaptureReleases {
    touches: Vec<u64>,
}

impl TouchGestureCaptureState {
    pub(crate) fn capture(&mut self, touch_id: u64, owner: Entity) {
        self.owners_by_touch.insert(touch_id, owner);
    }

    pub(crate) fn release_touch(&mut self, touch_id: u64) {
        self.owners_by_touch.remove(&touch_id);
    }

    pub(crate) fn release_owner(&mut self, owner: Entity) {
        self.owners_by_touch.retain(|_, current_owner| *current_owner != owner);
    }

    #[allow(dead_code)]
    pub(crate) fn owner(&self, touch_id: u64) -> Option<Entity> {
        self.owners_by_touch.get(&touch_id).copied()
    }
}

pub(crate) fn pointer_touch_id(pointer_id: PointerId) -> Option<u64> {
    match pointer_id {
        PointerId::Touch(touch_id) => Some(touch_id),
        PointerId::Mouse | PointerId::Custom(_) => None,
    }
}

pub(super) fn capture_touch_press(
    ev: On<Pointer<Press>>,
    captures: Query<(), With<ScrollGestureCapture>>,
    mut state: ResMut<TouchGestureCaptureState>,
) {
    if captures.contains(ev.entity)
        && let Some(touch_id) = pointer_touch_id(ev.pointer_id)
    {
        state.capture(touch_id, ev.entity);
    }
}

pub(super) fn release_touch_release(ev: On<Pointer<Release>>, mut pending: ResMut<PendingTouchGestureCaptureReleases>) {
    queue_pointer_touch_release(ev.pointer_id, &mut pending);
}

pub(super) fn release_touch_drag_end(
    ev: On<Pointer<DragEnd>>,
    mut pending: ResMut<PendingTouchGestureCaptureReleases>,
) {
    queue_pointer_touch_release(ev.pointer_id, &mut pending);
}

pub(super) fn release_touch_cancel(ev: On<Pointer<Cancel>>, mut pending: ResMut<PendingTouchGestureCaptureReleases>) {
    queue_pointer_touch_release(ev.pointer_id, &mut pending);
}

pub(super) fn cleanup_removed_gesture_capture(
    mut removed_captures: RemovedComponents<ScrollGestureCapture>,
    mut state: ResMut<TouchGestureCaptureState>,
) {
    for owner in removed_captures.read() {
        state.release_owner(owner);
    }
}

pub(super) fn release_ended_touch_captures(
    mut touch_events: MessageReader<TouchInput>,
    mut pending: ResMut<PendingTouchGestureCaptureReleases>,
) {
    for touch in touch_events.read() {
        if matches!(touch.phase, TouchPhase::Ended | TouchPhase::Canceled) {
            pending.touches.push(touch.id);
        }
    }
}

pub(super) fn flush_pending_touch_releases(
    mut pending: ResMut<PendingTouchGestureCaptureReleases>,
    mut state: ResMut<TouchGestureCaptureState>,
) {
    for touch_id in pending.touches.drain(..) {
        state.release_touch(touch_id);
    }
}

fn queue_pointer_touch_release(pointer_id: PointerId, pending: &mut PendingTouchGestureCaptureReleases) {
    if let Some(touch_id) = pointer_touch_id(pointer_id) {
        pending.touches.push(touch_id);
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::touch::{TouchInput, TouchPhase};
    use bevy::prelude::*;

    use super::{
        PendingTouchGestureCaptureReleases, ScrollGestureCapture, TouchGestureCaptureState,
        cleanup_removed_gesture_capture, flush_pending_touch_releases, pointer_touch_id, release_ended_touch_captures,
    };

    #[test]
    fn pointer_touch_id_returns_touch_for_touch_pointer() {
        assert_eq!(pointer_touch_id(bevy::picking::pointer::PointerId::Touch(7)), Some(7));
    }

    #[test]
    fn pointer_touch_id_ignores_mouse_pointer() {
        assert_eq!(pointer_touch_id(bevy::picking::pointer::PointerId::Mouse), None);
    }

    #[test]
    fn capture_state_tracks_owner_by_touch_id() {
        let owner_a = Entity::from_raw_u32(11).unwrap();
        let owner_b = Entity::from_raw_u32(22).unwrap();
        let mut state = TouchGestureCaptureState::default();

        state.capture(1, owner_a);
        state.capture(2, owner_b);

        assert_eq!(state.owner(1), Some(owner_a));
        assert_eq!(state.owner(2), Some(owner_b));

        state.capture(1, owner_b);

        assert_eq!(state.owner(1), Some(owner_b));
    }

    #[test]
    fn release_paths_are_idempotent() {
        let owner = Entity::from_raw_u32(33).unwrap();
        let mut state = TouchGestureCaptureState::default();

        state.capture(1, owner);
        state.release_touch(1);
        state.release_touch(1);
        state.release_owner(owner);
        state.release_owner(owner);

        assert_eq!(state.owner(1), None);
    }

    #[test]
    fn cleanup_removed_owner_releases_all_touches() {
        let mut app = App::new();
        app.init_resource::<TouchGestureCaptureState>()
            .add_systems(Update, cleanup_removed_gesture_capture);

        let owner = app.world_mut().spawn(ScrollGestureCapture).id();
        let other_owner = app.world_mut().spawn(ScrollGestureCapture).id();

        {
            let mut state = app.world_mut().resource_mut::<TouchGestureCaptureState>();
            state.capture(1, owner);
            state.capture(2, owner);
            state.capture(3, other_owner);
        }

        app.world_mut().despawn(owner);
        app.update();

        let state = app.world().resource::<TouchGestureCaptureState>();
        assert_eq!(state.owner(1), None);
        assert_eq!(state.owner(2), None);
        assert_eq!(state.owner(3), Some(other_owner));
    }

    #[test]
    fn raw_touch_end_releases_capture_without_pointer_release() {
        let mut app = App::new();
        app.init_resource::<TouchGestureCaptureState>()
            .init_resource::<PendingTouchGestureCaptureReleases>()
            .add_message::<TouchInput>()
            .add_systems(
                Update,
                (release_ended_touch_captures, flush_pending_touch_releases).chain(),
            );

        let owner = app.world_mut().spawn(ScrollGestureCapture).id();
        {
            let mut state = app.world_mut().resource_mut::<TouchGestureCaptureState>();
            state.capture(9, owner);
        }

        app.world_mut().write_message(TouchInput {
            phase: TouchPhase::Canceled,
            position: Vec2::ZERO,
            window: Entity::from_raw_u32(1).unwrap(),
            force: None,
            id: 9,
        });
        app.update();

        let state = app.world().resource::<TouchGestureCaptureState>();
        assert_eq!(state.owner(9), None);
    }

    #[test]
    fn pending_release_keeps_capture_until_flush() {
        let owner = Entity::from_raw_u32(44).unwrap();
        let mut state = TouchGestureCaptureState::default();
        let mut pending = PendingTouchGestureCaptureReleases::default();

        state.capture(5, owner);
        pending.touches.push(5);

        assert_eq!(state.owner(5), Some(owner));

        for touch_id in pending.touches.drain(..) {
            state.release_touch(touch_id);
        }

        assert_eq!(state.owner(5), None);
    }
}
