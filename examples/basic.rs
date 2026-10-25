//! Independent wheel/touch, native scrollbar capture, snap, navigation and controlled refresh.
use bevy::{
    prelude::*,
    ui_widgets::{ControlOrientation, Scrollbar, ScrollbarThumb},
};
use bevy_ui_scroll::pull_to_refresh::{PullToRefresh, PullToRefreshPlugin, PullToRefreshState, RefreshRequested};
use bevy_ui_scroll::{
    EnhancedScroll, ScrollBehavior, ScrollGestureCapture, ScrollPlugin, ScrollSnap, ScrollSnapAxis, ScrollSnapPoint,
    ScrollToOffset, ScrollToSnap, SmoothScroll, SnapTarget, StopScroll,
};

#[derive(Resource)]
struct Demo {
    viewport: Entity,
    snap: Entity,
    pages: Vec<Entity>,
    selected: usize,
}
#[derive(Component)]
struct RefreshTimer(Timer);
#[derive(Component)]
struct Status;

fn main() {
    demo_app().run();
}

fn demo_app() -> App {
    let mut app = App::new();
    app.add_plugins((DefaultPlugins, ScrollPlugin, PullToRefreshPlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, (keys, finish_refresh, status))
        .add_observer(start_refresh);
    app
}
fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    let root = commands
        .spawn((
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(20)),
                row_gap: px(16),
                ..default()
            },
            BackgroundColor(Color::srgb(0.06, 0.07, 0.09)),
        ))
        .id();
    commands.spawn((ChildOf(root), Text::new("Wheel / touch to scroll. Home / End: smooth position. Escape: cancel. Left / Right: snap. Pull down at the top: refresh."),
        TextFont { font_size: px(18).into(), ..default() }));
    let panels = commands
        .spawn((
            ChildOf(root),
            Node {
                width: percent(100),
                flex_grow: 1.,
                min_height: px(0),
                column_gap: px(20),
                ..default()
            },
        ))
        .id();
    let frame = commands
        .spawn((
            ChildOf(panels),
            Node {
                width: percent(55),
                height: percent(100),
                position_type: PositionType::Relative,
                overflow: Overflow::hidden(),
                ..default()
            },
        ))
        .id();
    let viewport = commands
        .spawn((
            ChildOf(frame),
            Name::new("enhanced_viewport"),
            EnhancedScroll,
            SmoothScroll,
            Node {
                width: percent(100),
                height: percent(100),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.1, 0.12, 0.16)),
        ))
        .id();
    let content = commands
        .spawn((
            ChildOf(viewport),
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                flex_shrink: 0.,
                ..default()
            },
        ))
        .id();
    for index in 0..50 {
        commands.spawn((
            ChildOf(content),
            Node {
                height: px(48),
                flex_shrink: 0.,
                padding: UiRect::all(px(12)),
                ..default()
            },
            Text::new(format!("Row {}", index + 1)),
            TextFont {
                font_size: px(16).into(),
                ..default()
            },
        ));
    }
    commands.spawn((
        ChildOf(frame),
        Scrollbar {
            target: viewport,
            orientation: ControlOrientation::Vertical,
            min_thumb_length: 40.,
        },
        Node {
            position_type: PositionType::Absolute,
            right: px(0),
            top: px(0),
            width: px(12),
            height: percent(100),
            ..default()
        },
        children![(
            ScrollbarThumb {
                border: UiRect::default(),
                border_radius: BorderRadius::all(px(4))
            },
            ScrollGestureCapture,
            BackgroundColor(Color::srgb(0.5, 0.6, 0.8))
        )],
    ));
    commands.spawn_scene(bsn! { @PullToRefresh { @target: viewport } ChildOf(frame) });
    let right = commands
        .spawn((
            ChildOf(panels),
            Node {
                flex_grow: 1.,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(20),
                ..default()
            },
        ))
        .id();
    let snap = commands
        .spawn((
            ChildOf(right),
            Name::new("snap_viewport"),
            ScrollSnap {
                axis: ScrollSnapAxis::X,
            },
            Node {
                width: percent(100),
                height: px(250),
                overflow: Overflow::scroll_x(),
                ..default()
            },
        ))
        .id();
    let mut pages = Vec::new();
    for (index, color) in [
        Color::srgb(0.18, 0.27, 0.5),
        Color::srgb(0.25, 0.42, 0.3),
        Color::srgb(0.45, 0.25, 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        pages.push(
            commands
                .spawn((
                    ChildOf(snap),
                    ScrollSnapPoint,
                    Node {
                        width: percent(100),
                        height: percent(100),
                        flex_shrink: 0.,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    BackgroundColor(color),
                    children![(
                        Text::new(format!("Snap page {}", index + 1)),
                        TextFont {
                            font_size: px(24).into(),
                            ..default()
                        }
                    )],
                ))
                .id(),
        );
    }
    commands.spawn((
        ChildOf(right),
        Status,
        Text::new(""),
        TextFont {
            font_size: px(16).into(),
            ..default()
        },
    ));
    commands.insert_resource(Demo {
        viewport,
        snap,
        pages,
        selected: 0,
    });
}
fn keys(input: Res<ButtonInput<KeyCode>>, mut demo: ResMut<Demo>, mut commands: Commands) {
    if input.just_pressed(KeyCode::Home) || input.just_pressed(KeyCode::End) {
        commands.trigger(ScrollToOffset {
            entity: demo.viewport,
            offset: Vec2::new(0., if input.just_pressed(KeyCode::Home) { 0. } else { 2400. }),
            behavior: ScrollBehavior::Smooth,
        });
    }
    if input.just_pressed(KeyCode::Escape) {
        commands.trigger(StopScroll { entity: demo.viewport });
        commands.trigger(StopScroll { entity: demo.snap });
    }
    if input.just_pressed(KeyCode::ArrowRight) || input.just_pressed(KeyCode::ArrowLeft) {
        demo.selected = if input.just_pressed(KeyCode::ArrowRight) {
            (demo.selected + 1).min(2)
        } else {
            demo.selected.saturating_sub(1)
        };
        commands.trigger(ScrollToSnap {
            entity: demo.snap,
            target: SnapTarget::Entity(demo.pages[demo.selected]),
            instant: false,
        });
    }
}
fn start_refresh(event: On<RefreshRequested>, mut controls: Query<&mut PullToRefreshState>, mut commands: Commands) {
    if let Ok(mut state) = controls.get_mut(event.entity) {
        state.refreshing = true;
        commands
            .entity(event.entity)
            .insert(RefreshTimer(Timer::from_seconds(1.5, TimerMode::Once)));
    }
}
fn finish_refresh(
    time: Res<Time>,
    mut controls: Query<(Entity, &mut PullToRefreshState, &mut RefreshTimer)>,
    mut commands: Commands,
) {
    for (entity, mut state, mut timer) in &mut controls {
        if timer.0.tick(time.delta()).just_finished() {
            state.refreshing = false;
            commands.entity(entity).remove::<RefreshTimer>();
        }
    }
}
fn status(
    demo: Res<Demo>,
    positions: Query<(&ScrollPosition, &ComputedNode)>,
    mut labels: Query<&mut Text, With<Status>>,
) {
    if let Ok((request, computed)) = positions.get(demo.viewport) {
        let text = format!(
            "Requested y: {:.1}\nActual y: {:.1}\n\nThe refresh overlay shows elasticity; native layout clamps content. Drag the scrollbar thumb to take over motion.",
            request.y,
            computed.scroll_position.y * computed.inverse_scale_factor()
        );
        for mut label in &mut labels {
            if label.0 != text {
                label.0.clone_from(&text);
            }
        }
    }
}
