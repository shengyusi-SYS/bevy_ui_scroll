//! Optional refresh interaction and presentation for an existing scroll viewport.
//! The caller owns request completion and projects `PullToRefreshState`.

use bevy::{input::touch::TouchPhase, window::RequestRedraw};
use bevy::{math::StableInterpolate, prelude::*};

#[derive(SceneComponent, Default, Clone)]
#[scene(PullToRefreshProps)]
pub struct PullToRefresh;

pub struct PullToRefreshProps {
    pub target: Entity,
    pub pull_label: String,
    pub release_label: String,
    pub refreshing_label: String,
    /// Required top overscroll distance in logical pixels.
    pub threshold: f32,
}

impl Default for PullToRefreshProps {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            pull_label: "Pull to refresh".into(),
            release_label: "Release to refresh".into(),
            refreshing_label: "Refreshing".into(),
            threshold: 48.0,
        }
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct PullToRefreshState {
    pub enabled: bool,
    pub refreshing: bool,
}

impl Default for PullToRefreshState {
    fn default() -> Self {
        Self {
            enabled: true,
            refreshing: false,
        }
    }
}

#[derive(EntityEvent)]
pub struct RefreshRequested {
    pub entity: Entity,
}

#[derive(Component, Clone)]
#[relationship(relationship_target = PullRefreshControls)]
#[require(PullToRefreshState, PullRefreshConfig, PullRefreshProgress, PullGesture)]
pub struct PullRefreshTarget(pub Entity);

impl Default for PullRefreshTarget {
    fn default() -> Self {
        Self(Entity::PLACEHOLDER)
    }
}

#[derive(Component, Default)]
#[relationship_target(relationship = PullRefreshTarget, linked_spawn)]
#[doc(hidden)]
pub struct PullRefreshControls(Vec<Entity>);

/// Threshold in logical pixels of requested top overscroll.
#[derive(Component, Clone, Copy)]
pub struct PullRefreshConfig {
    pub threshold: f32,
}
impl Default for PullRefreshConfig {
    fn default() -> Self {
        Self { threshold: 48.0 }
    }
}

/// Read-only presentation state for custom indicators; completion stays in PullToRefreshState.
#[derive(Component, Clone, Copy, Default, Debug, PartialEq)]
pub struct PullRefreshProgress {
    pub distance: f32,
    pub pulling: bool,
    pub armed: bool,
}

#[derive(Component, Clone, Default)]
struct PullLabels {
    pull: String,
    release: String,
    refreshing: String,
}

#[derive(Component, Clone, FromTemplate)]
pub struct PullToRefreshParts {
    pub body: Entity,
    pub caption: Entity,
    pub spinner: Entity,
    pub arrow: Entity,
}

#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PullGesture {
    touch: Option<u64>,
    armed: bool,
}

const REFRESH_DISARM_RATIO: f32 = 0.85;

/// Sent by the existing scroll session only for viewports with attached controls.
#[derive(EntityEvent)]
pub(crate) struct ScrollPullGesture {
    pub entity: Entity,
    pub touch: u64,
    pub phase: TouchPhase,
    pub vertical: bool,
}

impl PullToRefresh {
    fn scene(props: PullToRefreshProps) -> impl Scene {
        // Neutral default presentation; replace this scene with PullRefreshTarget +
        // PullRefreshConfig/State and render PullRefreshProgress for a custom indicator.
        bsn! {
            PullRefreshTarget({props.target})
            PullToRefreshState
            PullGesture
            PullRefreshConfig { threshold: {props.threshold} }
            PullLabels {
                pull: {props.pull_label},
                release: {props.release_label},
                refreshing: {props.refreshing_label},
            }
            PullToRefreshParts { body: #Body, caption: #Caption, spinner: #Progress, arrow: #Arrow }
            UiTransform::from_translation(Val2::px(0, -40))
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                top: px(0),
                left: px(0),
                width: percent(100),
                justify_content: JustifyContent::Center,
            }
            Pickable { should_block_lower: false, is_hoverable: false }
            ZIndex(2)
            Children [(
                #Body
                Node {
                    height: px(32),
                    min_width: px(144),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    column_gap: px(6),
                    padding: {UiRect::axes(px(12), px(6))},
                    border: px(1),
                    border_radius: BorderRadius::MAX,
                }
                BackgroundColor(Color::srgb(0.12, 0.12, 0.14))
                BorderColor::all(Color::srgb(0.3, 0.3, 0.34))
                Pickable { should_block_lower: false, is_hoverable: false }
                Children [
                    (
                        #Arrow
                        Node {
                            width: px(8),
                            height: px(8),
                            margin: {UiRect::axes(px(5), px(0))},
                            border: UiRect { top: px(0), right: px(2), bottom: px(2), left: px(0) },
                        }
                        UiTransform::from_rotation(Rot2::FRAC_PI_4)
                        BorderColor::all(Color::srgb(0.8, 0.8, 0.84))
                        Pickable { should_block_lower: false, is_hoverable: false }
                    ),
                    (
                        #Progress
                        Node {
                            display: Display::None,
                            width: px(18),
                            height: px(18),
                            border: UiRect { top: px(2), right: px(2), bottom: px(0), left: px(0) },
                            border_radius: BorderRadius::MAX,
                        }
                        UiTransform
                        IndicatorSpinnerSpeed
                        BorderColor::all(Color::srgb(0.65, 0.72, 1.0))
                        Pickable { should_block_lower: false, is_hoverable: false }
                    ),
                    (
                        #Caption
                        Text("")
                        TextFont { font_size: px(12) }
                        TextColor(Color::srgb(0.8, 0.8, 0.84))
                        Pickable { should_block_lower: false, is_hoverable: false }
                    ),
                ]
            )]
        }
    }
}

pub struct PullToRefreshPlugin;
impl Plugin for PullToRefreshPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RequestRedraw>()
            .add_observer(on_pull_session)
            .add_systems(Update, rotate_indicator_spinners)
            .add_systems(
                PostUpdate,
                (update_progress, update_presentation)
                    .chain()
                    .before(bevy::ui::UiSystems::Layout),
            );
    }
}

fn on_pull_session(
    event: On<ScrollPullGesture>,
    attachments: Query<&PullRefreshControls>,
    positions: Query<&ScrollPosition>,
    mut controls: Query<(&PullToRefreshState, &PullRefreshConfig, &mut PullGesture)>,
    mut commands: Commands,
) {
    let Ok(attachment) = attachments.get(event.entity) else {
        return;
    };
    let Ok(position) = positions.get(event.entity) else {
        return;
    };
    for &control in &attachment.0 {
        let Ok((state, config, mut gesture)) = controls.get_mut(control) else {
            continue;
        };
        let owns_touch = gesture.touch == Some(event.touch);
        if !state.enabled || state.refreshing || !event.vertical {
            if owns_touch {
                gesture.set_if_neq(PullGesture::default());
                let target = event.entity;
                commands.queue(move |world: &mut World| crate::release_refresh_pull(world, target));
            }
            continue;
        }
        match event.phase {
            TouchPhase::Moved if gesture.touch.is_none() || owns_touch => {
                let next = if position.y < 0.0 {
                    let distance = -position.y;
                    let threshold = config.threshold.max(1.0);
                    PullGesture {
                        touch: Some(event.touch),
                        armed: if owns_touch && gesture.armed {
                            distance > threshold * REFRESH_DISARM_RATIO
                        } else {
                            distance >= threshold
                        },
                    }
                } else {
                    PullGesture::default()
                };
                gesture.set_if_neq(next);
            }
            TouchPhase::Ended | TouchPhase::Canceled if owns_touch => {
                let request = event.phase == TouchPhase::Ended && gesture.armed;
                gesture.set_if_neq(PullGesture::default());
                let target = event.entity;
                commands.queue(move |world: &mut World| crate::release_refresh_pull(world, target));
                if request {
                    commands.trigger(RefreshRequested { entity: control });
                }
            }
            _ => {}
        }
    }
}

fn update_progress(
    mut controls: Query<(
        &PullRefreshTarget,
        &PullToRefreshState,
        &mut PullGesture,
        &mut PullRefreshProgress,
    )>,
    positions: Query<&ScrollPosition>,
    mut commands: Commands,
) {
    for (target, state, mut gesture, mut progress) in &mut controls {
        if !state.enabled || state.refreshing {
            if gesture.touch.is_some() {
                let entity = target.0;
                commands.queue(move |world: &mut World| crate::release_refresh_pull(world, entity));
            }
            gesture.set_if_neq(PullGesture::default());
        }
        let pulling = state.enabled && gesture.touch.is_some();
        progress.set_if_neq(PullRefreshProgress {
            distance: if pulling {
                positions.get(target.0).map_or(0.0, |position| (-position.y).max(0.0))
            } else {
                0.0
            },
            pulling,
            armed: pulling && gesture.armed,
        });
    }
}

fn update_presentation(
    mut controls: Query<
        (
            &PullToRefreshState,
            &PullLabels,
            &PullToRefreshParts,
            &PullRefreshConfig,
            &PullRefreshProgress,
            &mut Node,
            &mut UiTransform,
        ),
        With<PullToRefresh>,
    >,
    mut child_nodes: Query<(&mut Node, Option<&IndicatorSpinner>), Without<PullToRefresh>>,
    mut arrows: Query<&mut UiTransform, Without<PullToRefresh>>,
    mut captions: Query<&mut Text>,
    time: Res<Time>,
    mut redraw: MessageWriter<RequestRedraw>,
    mut commands: Commands,
) {
    let mut needs_redraw = false;
    for (state, labels, parts, config, progress, mut root, mut transform) in &mut controls {
        let busy = state.enabled && state.refreshing;
        let pulling = progress.pulling;
        if !busy && !pulling && root.display == Display::None {
            continue;
        }
        // Follow the existing rubber-band position while dragging; animate only
        // this overlay on release. Never animate Node geometry or the content tree.
        let distance = progress.distance;
        let target_y = if busy {
            8.0
        } else if pulling {
            -40.0 + 48.0 * (distance / config.threshold.max(1.0)).clamp(0.0, 1.2)
        } else {
            -40.0
        };
        let Val::Px(mut y) = transform.translation.y else {
            continue;
        };
        if pulling || !state.enabled {
            y = target_y;
        } else {
            y.smooth_nudge(&target_y, 24.0, time.delta_secs());
            if (y - target_y).abs() < 0.1 {
                y = target_y;
            }
        }
        let translation = Val2::px(0.0, y);
        if transform.translation != translation {
            transform.translation = translation;
            needs_redraw = true;
        }
        let shown = state.enabled && (busy || pulling || y > -40.0);
        let display = if shown { Display::Flex } else { Display::None };
        if root.display != display {
            root.display = display;
            needs_redraw = true;
        }
        // Keep the last caption and graphic during retraction: no brief flash
        // back to "pull" while a completed refresh is disappearing.
        if busy || pulling {
            let caption = if busy {
                &labels.refreshing
            } else if progress.armed {
                &labels.release
            } else {
                &labels.pull
            };
            if let Ok(mut text) = captions.get_mut(parts.caption) {
                if text.0 != *caption {
                    text.0.clone_from(caption);
                }
            }
            if let Ok((mut node, _)) = child_nodes.get_mut(parts.arrow) {
                let display = if busy { Display::None } else { Display::Flex };
                if node.display != display {
                    node.display = display;
                }
            }
        }
        if pulling && let Ok(mut arrow) = arrows.get_mut(parts.arrow) {
            let fraction = (distance / config.threshold.max(1.0)).clamp(0.0, 1.0);
            let next = Rot2::radians(std::f32::consts::FRAC_PI_4 - std::f32::consts::PI * fraction);
            if arrow.rotation != next {
                arrow.rotation = next;
                needs_redraw = true;
            }
        }
        if let Ok((mut node, spinner)) = child_nodes.get_mut(parts.spinner) {
            if busy || pulling || !shown {
                let display = if busy { Display::Flex } else { Display::None };
                if node.display != display {
                    node.display = display;
                }
            }
            if busy && spinner.is_none() {
                commands.entity(parts.spinner).insert(IndicatorSpinner);
            }
            if !busy && spinner.is_some() {
                commands.entity(parts.spinner).remove::<IndicatorSpinner>();
            }
        }
        needs_redraw |= busy || (shown && y != target_y);
    }
    if needs_redraw {
        redraw.write(RequestRedraw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        input::{
            ButtonInput,
            keyboard::KeyCode,
            mouse::MouseWheel,
            touch::{TouchInput, TouchPhase},
        },
        picking::{backend::HitData, hover::HoverMap, pointer::PointerId},
        window::RequestRedraw,
    };
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct Requests(Vec<Entity>);

    fn fixture() -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_resource::<Time>()
        .init_resource::<HoverMap>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Requests>()
        .add_message::<MouseWheel>()
        .add_message::<TouchInput>()
        .add_message::<RequestRedraw>()
        .add_plugins((crate::ScrollPlugin, PullToRefreshPlugin))
        .add_observer(|event: On<RefreshRequested>, mut requests: ResMut<Requests>| requests.0.push(event.entity));
        let window = app.world_mut().spawn_empty().id();
        let viewport = app
            .world_mut()
            .spawn((
                Node {
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollPosition::default(),
                crate::EnhancedScroll,
                ComputedNode::default(),
            ))
            .id();
        let control = app
            .world_mut()
            .spawn_scene(bsn! {
                @PullToRefresh {
                    @target: viewport,
                    @pull_label: "Pull",
                    @release_label: "Release",
                    @refreshing_label: "Refreshing",
                }
            })
            .unwrap()
            .id();
        app.world_mut()
            .resource_mut::<HoverMap>()
            .entry(PointerId::Touch(7))
            .or_default()
            .insert(viewport, HitData::new(Entity::PLACEHOLDER, 0.0, None, None));
        (app, window, viewport, control)
    }

    fn touch(app: &mut App, window: Entity, phase: TouchPhase, y: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(16));
        app.world_mut().write_message(TouchInput {
            phase,
            position: Vec2::new(0.0, y),
            window,
            force: None,
            id: 7,
        });
        app.update();
    }

    #[test]
    fn pull_indicator_follows_distance_and_retracts_without_relaying_out_content() {
        // T2: gesture, visual update and real scroll return must cooperate.
        let (mut app, window, viewport, control) = fixture();
        touch(&mut app, window, TouchPhase::Started, 0.0);
        touch(&mut app, window, TouchPhase::Moved, 40.0);
        let first = app.world().get::<UiTransform>(control).unwrap().translation.y;
        let parts = app.world().get::<PullToRefreshParts>(control).unwrap().clone();
        let first_angle = app
            .world()
            .get::<UiTransform>(parts.arrow)
            .unwrap()
            .rotation
            .as_radians();
        let layout_tick = app.world().entity(control).get_ref::<Node>().unwrap().last_changed();
        let content_tick = app.world().entity(viewport).get_ref::<Node>().unwrap().last_changed();
        touch(&mut app, window, TouchPhase::Moved, 120.0);
        let second = app.world().get::<UiTransform>(control).unwrap().translation.y;
        let second_angle = app
            .world()
            .get::<UiTransform>(parts.arrow)
            .unwrap()
            .rotation
            .as_radians();
        assert!(!app.world().get::<PullRefreshProgress>(control).unwrap().armed);
        assert!(
            second_angle < first_angle - 0.05,
            "arrow must show progress before arming"
        );
        let (Val::Px(first), Val::Px(second)) = (first, second) else {
            panic!("indicator uses logical pixel translation");
        };
        assert!(
            second > first,
            "indicator must follow distance before the release threshold"
        );
        assert_eq!(
            app.world().entity(control).get_ref::<Node>().unwrap().last_changed(),
            layout_tick
        );
        assert_eq!(
            app.world().entity(viewport).get_ref::<Node>().unwrap().last_changed(),
            content_tick
        );
        touch(&mut app, window, TouchPhase::Canceled, 120.0);
        assert_eq!(
            app.world().get::<Node>(control).unwrap().display,
            Display::Flex,
            "cancellation retracts instead of disappearing in one frame"
        );
        for _ in 0..80 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_millis(16));
            app.update();
        }
        assert_eq!(app.world().get::<Node>(control).unwrap().display, Display::None);
        assert!(app.world().resource::<Requests>().0.is_empty());
        let settled_tick = app
            .world()
            .entity(control)
            .get_ref::<UiTransform>()
            .unwrap()
            .last_changed();
        app.world_mut().resource_mut::<Messages<RequestRedraw>>().clear();
        app.update();
        assert_eq!(
            app.world()
                .entity(control)
                .get_ref::<UiTransform>()
                .unwrap()
                .last_changed(),
            settled_tick
        );
        assert!(app.world().resource::<Messages<RequestRedraw>>().is_empty());
    }

    #[test]
    fn refresh_hysteresis_keeps_one_request_and_allows_retreat() {
        // T2: one real touch session retains qualification in the retreat band;
        // the next session can retreat further and release without refreshing.
        let (mut app, window, viewport, control) = fixture();
        for (retreat_to, expected_armed) in [(45.0, true), (38.0, false)] {
            touch(&mut app, window, TouchPhase::Started, 0.0);
            touch(&mut app, window, TouchPhase::Moved, 400.0);
            assert!(app.world().get::<PullRefreshProgress>(control).unwrap().armed);
            // Calibrate finger travel against the existing resistance mapping;
            // the expected requested distances and qualification stay fixed above.
            let distance = -app.world().get::<ScrollPosition>(viewport).unwrap().y;
            let finger_y = 400.0 - (distance - retreat_to);
            touch(&mut app, window, TouchPhase::Moved, finger_y);
            let progress = app.world().get::<PullRefreshProgress>(control).unwrap();
            assert!((progress.distance - retreat_to).abs() < 0.01);
            assert_eq!(progress.armed, expected_armed, "retreat to {retreat_to}px");
            touch(&mut app, window, TouchPhase::Ended, finger_y);
            touch(&mut app, window, TouchPhase::Ended, finger_y);
            assert_eq!(app.world().resource::<Requests>().0, vec![control]);
            for _ in 0..80 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(Duration::from_millis(16));
                app.update();
            }
            assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
            assert!(!app.world().get::<PullRefreshProgress>(control).unwrap().pulling);
        }
    }

    #[test]
    fn refresh_behavior_does_not_require_default_indicator_labels() {
        let (mut app, window, viewport, control) = fixture();
        app.world_mut()
            .entity_mut(control)
            .remove::<(PullLabels, PullToRefreshParts)>();
        touch(&mut app, window, TouchPhase::Started, 0.0);
        touch(&mut app, window, TouchPhase::Moved, 400.0);
        assert!(app.world().get::<ScrollPosition>(viewport).unwrap().y < -48.0);
        touch(&mut app, window, TouchPhase::Ended, 400.0);
        assert_eq!(app.world().resource::<Requests>().0, vec![control]);
    }

    #[test]
    fn top_pull_releases_one_refresh_through_the_real_scroll_session() {
        // T2: real HoverMap, TouchInput, target/direction selection and scroll observer.
        let (mut app, window, viewport, control) = fixture();
        touch(&mut app, window, TouchPhase::Started, 0.0);
        touch(&mut app, window, TouchPhase::Moved, 400.0);
        assert!(app.world().get::<ScrollPosition>(viewport).unwrap().y < -48.0);
        touch(&mut app, window, TouchPhase::Ended, 400.0);
        assert_eq!(app.world().resource::<Requests>().0, vec![control]);
        touch(&mut app, window, TouchPhase::Ended, 400.0);
        assert_eq!(app.world().resource::<Requests>().0, vec![control]);
    }

    #[test]
    fn short_and_canceled_pulls_return_without_requesting_refresh() {
        for (distance, phase) in [(40.0, TouchPhase::Ended), (400.0, TouchPhase::Canceled)] {
            let (mut app, window, viewport, control) = fixture();
            touch(&mut app, window, TouchPhase::Started, 0.0);
            touch(&mut app, window, TouchPhase::Moved, distance);
            touch(&mut app, window, phase, distance);
            assert!(app.world().resource::<Requests>().0.is_empty());
            assert_eq!(
                *app.world().get::<PullGesture>(control).unwrap(),
                PullGesture::default()
            );
            for _ in 0..80 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(Duration::from_millis(16));
                app.update();
            }
            assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
        }
    }

    #[test]
    fn batched_pull_finishes_via_the_existing_spring_without_an_active_touch() {
        let (mut app, window, viewport, control) = fixture();
        for (phase, y) in [
            (TouchPhase::Started, 0.0),
            (TouchPhase::Moved, 400.0),
            (TouchPhase::Ended, 400.0),
        ] {
            app.world_mut().write_message(TouchInput {
                phase,
                position: Vec2::new(0.0, y),
                window,
                force: None,
                id: 7,
            });
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(16));
        app.update();
        assert_eq!(app.world().resource::<Requests>().0, vec![control]);
        for _ in 0..80 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(Duration::from_millis(16));
            app.update();
        }
        assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
    }

    #[test]
    fn captured_control_gesture_does_not_scroll_or_refresh_its_viewport() {
        use bevy::{
            camera::NormalizedRenderTarget,
            picking::{
                events::{Pointer, Press},
                pointer::{Location, PointerButton},
            },
            window::WindowRef,
        };
        let (mut app, window, viewport, _) = fixture();
        let slider = app
            .world_mut()
            .spawn((crate::ScrollGestureCapture, ChildOf(viewport)))
            .id();
        touch(&mut app, window, TouchPhase::Started, 0.0);
        app.world_mut().trigger(Pointer::new(
            PointerId::Touch(7),
            Location {
                target: NormalizedRenderTarget::Window(WindowRef::Entity(window).normalize(None).unwrap()),
                position: Vec2::ZERO,
            },
            Press {
                button: PointerButton::Primary,
                hit: HitData::new(window, 0.0, None, None),
                count: 1,
            },
            slider,
        ));
        touch(&mut app, window, TouchPhase::Moved, 400.0);
        touch(&mut app, window, TouchPhase::Ended, 400.0);
        assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
        assert!(app.world().resource::<Requests>().0.is_empty());
    }

    #[test]
    fn inertia_and_idle_presentation_do_not_request_refresh_or_rewrite_layout() {
        let (mut app, _, viewport, control) = fixture();
        app.update();
        let parts = app.world().get::<PullToRefreshParts>(control).unwrap().clone();
        let before = app.world().entity(viewport).get_ref::<Node>().unwrap().last_changed();
        let root_before = app.world().entity(control).get_ref::<Node>().unwrap().last_changed();
        let text_before = app
            .world()
            .entity(parts.caption)
            .get_ref::<Text>()
            .unwrap()
            .last_changed();
        app.world_mut().get_mut::<ScrollPosition>(viewport).unwrap().y = -80.0;
        app.update();
        assert!(app.world().resource::<Requests>().0.is_empty());
        assert_eq!(
            app.world().entity(viewport).get_ref::<Node>().unwrap().last_changed(),
            before
        );
        assert_eq!(
            app.world().entity(control).get_ref::<Node>().unwrap().last_changed(),
            root_before
        );
        assert_eq!(
            app.world()
                .entity(parts.caption)
                .get_ref::<Text>()
                .unwrap()
                .last_changed(),
            text_before
        );
        assert_eq!(app.world().get::<Node>(control).unwrap().display, Display::None);

        app.world_mut()
            .get_mut::<PullToRefreshState>(control)
            .unwrap()
            .refreshing = true;
        app.update();
        assert_eq!(app.world().get::<Text>(parts.caption).unwrap().0, "Refreshing");
        assert_eq!(app.world().get::<Node>(control).unwrap().display, Display::Flex);
        assert!(app.world().get::<super::IndicatorSpinner>(parts.spinner).is_some());
        let busy_tick = app.world().entity(control).get_ref::<Node>().unwrap().last_changed();
        app.update();
        assert_eq!(
            app.world().entity(control).get_ref::<Node>().unwrap().last_changed(),
            busy_tick
        );
        app.world_mut().get_mut::<PullToRefreshState>(control).unwrap().enabled = false;
        app.update();
        assert_eq!(app.world().get::<Node>(control).unwrap().display, Display::None);
        assert!(app.world().get::<super::IndicatorSpinner>(parts.spinner).is_none());
        assert_eq!(
            app.world().entity(viewport).get_ref::<Node>().unwrap().last_changed(),
            before
        );
    }
}

#[derive(Component, Default, Clone)]
struct IndicatorSpinner;

#[derive(Component, Clone)]
struct IndicatorSpinnerSpeed(f32);
impl Default for IndicatorSpinnerSpeed {
    fn default() -> Self {
        Self(std::f32::consts::TAU)
    }
}
fn rotate_indicator_spinners(
    mut indicators: Query<(&mut UiTransform, &IndicatorSpinnerSpeed), With<IndicatorSpinner>>,
    time: Res<Time>,
) {
    for (mut transform, speed) in &mut indicators {
        transform.rotation *= Rot2::radians(speed.0 * time.delta_secs());
    }
}
