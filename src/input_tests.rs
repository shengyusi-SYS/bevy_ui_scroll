use super::*;
use bevy::{input::mouse::MouseScrollUnit, prelude::*};

fn viewport(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Node {
                overflow: Overflow::scroll_y(),
                ..default()
            },
            ScrollPosition::default(),
            ComputedNode {
                size: Vec2::splat(100.0),
                content_size: Vec2::new(100.0, 600.0),
                inverse_scale_factor: 1.0,
                ..default()
            },
        ))
        .id()
}
fn wheel(app: &mut App, entity: Entity, unit: MouseScrollUnit) {
    app.world_mut().trigger(crate::Scroll {
        entity,
        delta: Vec2::new(0.0, 30.0),
        original_delta: Vec2::new(0.0, 30.0),
        source: ScrollSource::Wheel,
        unit,
    });
}
#[test]
fn unmarked_native_viewport_is_not_claimed_by_enhancement() {
    let mut app = App::new();
    app.add_plugins(ScrollPlugin);
    let entity = viewport(&mut app);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 0.0);
}
#[test]
fn pixel_wheel_is_direct_even_when_line_smoothing_is_enabled() {
    let mut app = App::new();
    app.add_plugins(ScrollPlugin);
    let entity = viewport(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert((EnhancedScroll, SmoothScroll));
    wheel(&mut app, entity, MouseScrollUnit::Pixel);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 30.0);
}

fn pointer_wheel(app: &mut App, entity: Entity, unit: MouseScrollUnit, y: f32) {
    use bevy::{
        camera::NormalizedRenderTarget,
        picking::{
            backend::HitData,
            pointer::{Location, PointerId},
        },
        window::WindowRef,
    };
    let window = app.world_mut().spawn(Window::default()).id();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: NormalizedRenderTarget::Window(WindowRef::Entity(window).normalize(None).unwrap()),
            position: Vec2::ZERO,
        },
        bevy::picking::events::Scroll {
            unit,
            x: 0.0,
            y,
            hit: HitData::new(window, 0.0, None, None),
            phase: bevy::input::touch::TouchPhase::Moved,
        },
        entity,
    ));
    app.world_mut().flush();
}

#[test]
fn native_and_enhanced_descendants_each_consume_the_pointer_wheel_once() {
    let mut app = App::new();
    app.add_plugins((ScrollPlugin, bevy::ui_widgets::ScrollAreaPlugin));
    let outer = viewport(&mut app);
    app.world_mut().entity_mut(outer).insert(EnhancedScroll);
    let native = viewport(&mut app);
    app.world_mut()
        .entity_mut(native)
        .insert((bevy::ui_widgets::ScrollArea, ChildOf(outer)));
    let leaf = app.world_mut().spawn(ChildOf(native)).id();
    pointer_wheel(&mut app, leaf, MouseScrollUnit::Pixel, -30.0);
    assert_eq!(app.world().get::<ScrollPosition>(native).unwrap().y, 30.0);
    assert_eq!(app.world().get::<ScrollPosition>(outer).unwrap().y, 0.0);
    pointer_wheel(&mut app, outer, MouseScrollUnit::Pixel, -30.0);
    assert_eq!(app.world().get::<ScrollPosition>(outer).unwrap().y, 30.0);
}

#[test]
fn line_wheel_at_a_smooth_child_boundary_reaches_its_parent() {
    let mut app = App::new();
    app.add_plugins(ScrollPlugin);
    let parent = viewport(&mut app);
    app.world_mut().entity_mut(parent).insert(EnhancedScroll);
    let child = viewport(&mut app);
    app.world_mut()
        .entity_mut(child)
        .insert((SmoothScroll, ChildOf(parent), ScrollPosition(Vec2::new(0.0, 500.0))));
    wheel(&mut app, child, MouseScrollUnit::Line);
    assert_eq!(app.world().get::<ScrollPosition>(parent).unwrap().y, 30.0);
}

fn running_app() -> App {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<bevy::picking::hover::HoverMap>()
        .add_message::<bevy::input::touch::TouchInput>()
        .add_plugins(ScrollPlugin);
    app
}
fn frame(app: &mut App, millis: u64) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(millis));
    app.update();
}

#[test]
fn touch_fling_completes_at_the_same_endpoint_across_frame_steps() {
    use bevy::{
        input::touch::{TouchInput, TouchPhase},
        picking::{backend::HitData, hover::HoverMap, pointer::PointerId},
    };
    for steps in [
        &[1. / 60.][..],
        &[1. / 120.][..],
        &[0.001, 0.037, 0.005, 0.019, 0.007][..],
    ] {
        let mut app = running_app();
        let entity = viewport(&mut app);
        app.world_mut().entity_mut(entity).insert(EnhancedScroll);
        app.world_mut().get_mut::<ComputedNode>(entity).unwrap().content_size.y = 10000.;
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Touch(41))
            .or_default()
            .insert(entity, HitData::new(Entity::PLACEHOLDER, 0., None, None));
        for (phase, y, millis) in [
            (TouchPhase::Started, 50., 0),
            (TouchPhase::Moved, 0., 25),
            (TouchPhase::Moved, -50., 25),
            (TouchPhase::Moved, -100., 25),
            (TouchPhase::Moved, -150., 25),
            (TouchPhase::Ended, -150., 1),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::new(50., y),
                window: Entity::PLACEHOLDER,
                force: None,
                id: 41,
            });
            frame(&mut app, millis);
        }
        for i in 0..250 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f64(steps[i % steps.len()]));
            app.update();
        }
        // Four 50px/25ms moves release at 2000px/s. Android's mdpi reference
        // (160ppi, friction 0.015) travels 647.3988px, plus the 200px drag.
        // Freeze that reference value independently of the production helper.
        let expected = 847.3988;
        let actual = app.world().get::<ScrollPosition>(entity).unwrap().y;
        assert!(
            (actual - expected).abs() <= 0.5,
            "steps={steps:?}: {actual} vs {expected}"
        );
    }
}

#[test]
fn line_wheel_retargets_without_restarting_velocity() {
    let mut app = running_app();
    let entity = viewport(&mut app);
    app.world_mut().entity_mut(entity).insert(SmoothScroll);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    frame(&mut app, 100);
    let p0 = app.world().get::<ScrollPosition>(entity).unwrap().y;
    frame(&mut app, 1);
    let p1 = app.world().get::<ScrollPosition>(entity).unwrap().y;
    let before = (p1 - p0) / 0.001;
    wheel(&mut app, entity, MouseScrollUnit::Line);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, p1);
    frame(&mut app, 1);
    let p2 = app.world().get::<ScrollPosition>(entity).unwrap().y;
    let after = (p2 - p1) / 0.001;
    assert!((after - before).abs() < 50., "velocity jumped from {before} to {after}");
    frame(&mut app, 500);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 60.);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    frame(&mut app, 50);
    let turning = app.world().get::<ScrollPosition>(entity).unwrap().y;
    app.world_mut().trigger(crate::Scroll {
        entity,
        delta: Vec2::new(0., -90.),
        original_delta: Vec2::new(0., -90.),
        source: ScrollSource::Wheel,
        unit: MouseScrollUnit::Line,
    });
    frame(&mut app, 100);
    assert!(app.world().get::<ScrollPosition>(entity).unwrap().y < turning);
    frame(&mut app, 500);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 0.);
}

#[test]
fn line_wheel_settles_at_large_offsets() {
    use bevy::window::RequestRedraw;

    let mut app = running_app();
    let entity = viewport(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert((SmoothScroll, ScrollPosition(Vec2::new(0., 1_048_576.))));
    app.world_mut().get_mut::<ComputedNode>(entity).unwrap().content_size.y = 2_000_000.;
    wheel(&mut app, entity, MouseScrollUnit::Line);
    for _ in 0..120 {
        frame(&mut app, 16);
    }
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 1_048_606.);
    app.world_mut().resource_mut::<Messages<RequestRedraw>>().clear();
    frame(&mut app, 16);
    assert!(app.world().resource::<Messages<RequestRedraw>>().is_empty());
}

#[test]
fn reserved_scrollbar_extent_matches_native_range() {
    let mut app = running_app();
    app.init_resource::<bevy::ui::ui_surface::UiSurface>()
        .init_resource::<bevy::text::FontCx>()
        .add_systems(
            PostUpdate,
            bevy::ui::ui_layout_system.in_set(bevy::ui::UiSystems::Layout),
        );
    let entity = app
        .world_mut()
        .spawn((
            EnhancedScroll,
            Node {
                width: Val::Px(320.),
                height: Val::Px(200.),
                overflow: Overflow::scroll(),
                scrollbar_width: 12.,
                ..default()
            },
            ScrollPosition(Vec2::splat(5000.)),
        ))
        .id();
    let content = app
        .world_mut()
        .spawn((
            Node {
                width: Val::Px(600.),
                height: Val::Px(1000.),
                flex_shrink: 0.,
                ..default()
            },
            ChildOf(entity),
        ))
        .id();
    let point = app
        .world_mut()
        .spawn((
            ScrollSnapPoint,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(900.),
                width: Val::Px(100.),
                height: Val::Px(100.),
                ..default()
            },
            ChildOf(content),
        ))
        .id();
    app.update();
    let native_end = app.world().get::<ComputedNode>(entity).unwrap().scroll_position;
    assert_eq!(native_end, Vec2::new(292., 812.));
    for behavior in [ScrollBehavior::Instant, ScrollBehavior::Smooth] {
        app.world_mut().get_mut::<ScrollPosition>(entity).unwrap().0 = Vec2::ZERO;
        app.world_mut().trigger(ScrollToOffset {
            entity,
            offset: Vec2::splat(5000.),
            behavior,
        });
        app.world_mut().flush();
        frame(&mut app, 300);
        assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().0, native_end);
    }
    app.world_mut().get_mut::<ScrollPosition>(entity).unwrap().y = native_end.y - 5.;
    wheel(&mut app, entity, MouseScrollUnit::Pixel);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, native_end.y);
    app.world_mut().entity_mut(entity).insert(ScrollSnap {
        axis: ScrollSnapAxis::Y,
    });
    app.world_mut().trigger(ScrollToSnap {
        entity,
        target: SnapTarget::Entity(point),
        instant: true,
    });
    app.world_mut().flush();
    frame(&mut app, 16);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, native_end.y);
}
#[test]
fn consecutive_line_wheels_do_not_replay_already_consumed_distance() {
    let mut app = running_app();
    let entity = viewport(&mut app);
    app.world_mut().entity_mut(entity).insert(SmoothScroll);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    frame(&mut app, 100);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    frame(&mut app, 250);
    assert!((app.world().get::<ScrollPosition>(entity).unwrap().y - 60.0).abs() < 0.01);
}
#[test]
fn offset_navigation_replaces_motion_and_real_input_cancels_its_animation_or_wait() {
    let mut app = running_app();
    let entity = viewport(&mut app);
    app.world_mut().entity_mut(entity).insert(SmoothScroll);
    wheel(&mut app, entity, MouseScrollUnit::Line);
    app.world_mut().trigger(ScrollToOffset {
        entity,
        offset: Vec2::new(0., 300.),
        behavior: ScrollBehavior::Instant,
    });
    app.world_mut().flush();
    frame(&mut app, 250);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 300.);
    app.world_mut().trigger(ScrollToOffset {
        entity,
        offset: Vec2::ZERO,
        behavior: ScrollBehavior::Smooth,
    });
    app.world_mut().flush();
    frame(&mut app, 50);
    let current = app.world().get::<ScrollPosition>(entity).unwrap().y;
    wheel(&mut app, entity, MouseScrollUnit::Pixel);
    app.world_mut().flush();
    frame(&mut app, 250);
    assert!((app.world().get::<ScrollPosition>(entity).unwrap().y - (current + 30.)).abs() < 0.01);
    app.world_mut().get_mut::<ComputedNode>(entity).unwrap().size = Vec2::ZERO;
    app.world_mut().trigger(ScrollToOffset {
        entity,
        offset: Vec2::ZERO,
        behavior: ScrollBehavior::Instant,
    });
    app.world_mut().flush();
    app.world_mut().trigger(ScrollInput {
        entity,
        kind: ScrollInputKind::Touch,
    });
    app.world_mut().flush();
    app.world_mut().get_mut::<ComputedNode>(entity).unwrap().size = Vec2::splat(100.);
    frame(&mut app, 16);
    assert!((app.world().get::<ScrollPosition>(entity).unwrap().y - (current + 30.)).abs() < 0.01);
}

#[test]
fn native_thumb_press_takes_over_motion_even_when_native_stops_bubbling() {
    use bevy::{
        camera::NormalizedRenderTarget,
        picking::{
            backend::HitData,
            pointer::{Location, PointerButton, PointerId},
        },
        ui_widgets::{ControlOrientation, Scrollbar, ScrollbarPlugin, ScrollbarThumb},
        window::WindowRef,
    };
    let mut app = running_app();
    app.init_resource::<UiScale>().add_plugins(ScrollbarPlugin);
    let entity = viewport(&mut app);
    app.world_mut().entity_mut(entity).insert(EnhancedScroll);
    let bar = app
        .world_mut()
        .spawn(Scrollbar {
            target: entity,
            orientation: ControlOrientation::Vertical,
            min_thumb_length: 40.,
        })
        .id();
    let thumb = app
        .world_mut()
        .spawn((
            ScrollbarThumb {
                border: UiRect::default(),
                border_radius: BorderRadius::ZERO,
            },
            ChildOf(bar),
        ))
        .id();
    let window = app.world_mut().spawn(Window::default()).id();
    app.world_mut().trigger(ScrollToOffset {
        entity,
        offset: Vec2::new(0., 300.),
        behavior: ScrollBehavior::Smooth,
    });
    app.world_mut().flush();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: NormalizedRenderTarget::Window(WindowRef::Entity(window).normalize(None).unwrap()),
            position: Vec2::ZERO,
        },
        Press {
            button: PointerButton::Primary,
            hit: HitData::new(window, 0., None, None),
            count: 1,
        },
        thumb,
    ));
    app.world_mut().flush();
    frame(&mut app, 300);
    assert_eq!(app.world().get::<ScrollPosition>(entity).unwrap().y, 0.);
}

#[test]
fn cancelled_overscroll_settles_once_at_the_nonzero_snap_point() {
    use bevy::{
        input::touch::{TouchInput, TouchPhase},
        math::Affine2,
        picking::{backend::HitData, hover::HoverMap, pointer::PointerId},
    };
    let mut app = running_app();
    let entity = viewport(&mut app);
    app.world_mut().entity_mut(entity).insert((
        ScrollSnap {
            axis: ScrollSnapAxis::Y,
        },
        UiGlobalTransform::from(Affine2::from_translation(Vec2::splat(50.))),
    ));
    app.world_mut().spawn((
        Node::default(),
        ScrollSnapPoint,
        ComputedNode {
            size: Vec2::splat(20.),
            ..default()
        },
        UiGlobalTransform::from(Affine2::from_translation(Vec2::new(50., 110.))),
        ChildOf(entity),
    ));
    app.world_mut()
        .resource_mut::<HoverMap>()
        .entry(PointerId::Touch(9))
        .or_default()
        .insert(entity, HitData::new(Entity::PLACEHOLDER, 0., None, None));
    for (phase, y) in [
        (TouchPhase::Started, 50.),
        (TouchPhase::Moved, 150.),
        (TouchPhase::Canceled, 150.),
    ] {
        app.world_mut().write_message(TouchInput {
            phase,
            position: Vec2::new(50., y),
            window: Entity::PLACEHOLDER,
            force: None,
            id: 9,
        });
        frame(&mut app, 16);
    }
    let mut last = app.world().get::<ScrollPosition>(entity).unwrap().y;
    for _ in 0..100 {
        frame(&mut app, 16);
        let current = app.world().get::<ScrollPosition>(entity).unwrap().y;
        assert!(
            current >= last - 0.01,
            "snap must not fall back toward the boundary: {last} -> {current}"
        );
        last = current;
    }
    assert!(
        (last - 100.).abs() < 0.5,
        "expected the actual first point at 100, got {last}"
    );
}
