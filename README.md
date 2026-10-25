# bevy_ui_scroll

[简体中文](README.zh-CN.md) · [Developer guide](docs/development.md)

Add smooth scrolling, touch inertia, snap points and pull-to-refresh to Bevy UI.
Attach small components to ordinary Bevy nodes and control scrolling with events.

## Install

Requires Rust 1.95 or newer and Bevy 0.19.1.

```toml
[dependencies]
bevy = { version = "=0.19.1", default-features = false, features = ["ui", "bevy_ui_widgets"] }
bevy_ui_scroll = { git = "https://github.com/shengyusi-SYS/bevy_ui_scroll.git", tag = "v0.1.0" }
```

## A scrollable panel

```rust
use bevy::prelude::*;
use bevy_ui_scroll::{EnhancedScroll, ScrollPlugin, SmoothScroll};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ScrollPlugin))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        EnhancedScroll,
        SmoothScroll,
        Node {
            width: px(360),
            height: px(400),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        children![(
            Node { height: px(1600), flex_shrink: 0.0, ..default() },
            Text::new("Scroll with the wheel or drag with a finger"),
        )],
    ));
}
```

`EnhancedScroll` enables wheel and touch handling and supplies `ScrollPosition`.
Use it as the viewport's input marker. Use Bevy's `ScrollArea` on viewports that
should keep native wheel handling. `SmoothScroll` adds animation to line-based
mouse wheels; pixel-based trackpad input keeps its direct movement.

For horizontal scrolling, use `Overflow::scroll_x()`. Nested enhanced viewports
hand input to an ancestor at their boundary. Add `OverscrollBehavior::Contain`
to keep it within a viewport. An event that first reaches the boundary is consumed
there; the next outward event can move the ancestor.

## Move and stop

Trigger these events through `Commands` or `World`:

| Event | Result |
| --- | --- |
| `ScrollToOffset { entity, offset, behavior }` | Move to a logical-pixel `Vec2` offset with `ScrollBehavior::Instant` or `Smooth` |
| `StopScroll { entity }` | Stop pending navigation, wheel animation, inertia, bounce and snap |
| `ScrollInput { entity, kind }` | Observe actual wheel, touch or scrollbar input, including input at a boundary |

Navigation waits for layout, clamps to the available range and replaces the
previous request. New user input takes over. When your application resets an
offset or starts its own drag scrolling, send `StopScroll` before writing
`ScrollPosition`. After stopping, idle snapping resumes with the next input or
navigation command.

`ScrollPosition` holds the requested offset. Read
`ComputedNode.scroll_position * ComputedNode::inverse_scale_factor()` for the
offset used by the last layout. During a touch pull, the requested offset can
extend past the edge while native layout keeps the content inside its bounds.

`Scroll` is the normalized, bubbling delta event. It carries the remaining
`delta`, `original_delta`, `source` and wheel `unit`. A wheel line is 60 logical
pixels. `ScrollInput` is the convenient hook for canceling application navigation.

## Snap to pages or items

Add `ScrollSnap { axis: ScrollSnapAxis::X }` (or `Y`) to a viewport and
`ScrollSnapPoint` to its snap targets. Trigger
`ScrollToSnap { entity, target: SnapTarget::Entity(point), instant: false }`.

`SnapTarget::Index(n)` follows depth-first child declaration order, skipping nested
snap subtrees. `ScrollSnapStatus` reports the selected entity and whether it has
settled after layout. Commands wait for a nonzero viewport and resolve against
the latest geometry. Missing targets are ignored; new input or `StopScroll`
cancels a waiting request.

## Pull to refresh

Install `pull_to_refresh::PullToRefreshPlugin` alongside `ScrollPlugin`. Place the
default indicator in a positioned frame containing the scrolling viewport:

```rust,ignore
use bevy_ui_scroll::pull_to_refresh::*;

commands.spawn_scene(bsn! {
    @PullToRefresh {
        @target: viewport,
        @pull_label: "Pull to refresh",
        @release_label: "Release to refresh",
        @refreshing_label: "Refreshing",
        @threshold: 48.0,
    }
    ChildOf(frame)
});
```

Observe `RefreshRequested` on the control entity. Start your work and set its
`PullToRefreshState.refreshing` to `true`; set it to `false` when the work finishes.
Set `enabled` according to whether the control is active.

The threshold measures the resisted top pull in logical pixels. A qualifying
touch release emits one request. After reaching the threshold, qualification
lasts until the pull retreats to 85% of it. This also works with short content.

To draw your own indicator, spawn `PullRefreshTarget(viewport)` with an optional
`PullRefreshConfig`, then read `PullRefreshProgress { distance, pulling, armed }`.
To style the default indicator, use the entity references in `PullToRefreshParts`
for its body, caption, arrow and spinner. The indicator supplies the elastic
visual feedback while the content keeps native layout.

## Scrollbars and virtual lists

Bevy's `Scrollbar` and `ScrollbarThumb` work with enhanced viewports. Add
`ScrollGestureCapture` to a touch-operated thumb or slider so it owns that touch
gesture.

For long lists, combine this crate with
[bevy_ui_virtual_list](https://github.com/shengyusi-SYS/bevy_ui_virtual_list).
Observe its `PooledListScrollTarget`, call `take_over()`, then send `ScrollToOffset`
for the supplied offset. Forward `ScrollInput` to `PooledListScrollInput` so new
input cancels waiting index navigation. Install one motion adapter per viewport.

## Try the example

```sh
git clone https://github.com/shengyusi-SYS/bevy_ui_scroll.git
cd bevy_ui_scroll
cargo run --example basic
```

The [example](examples/basic.rs) includes a scroll panel, native scrollbar, snap
pages and a refresh indicator. Home/End move the panel, Escape stops it, and
Left/Right select a page. Pull down at the top to start the demo's timed refresh.
Build setup, tests and the source map are in the [developer guide](docs/development.md).

## License

Original code: [MIT](LICENSE). The adapted Android fling and Chromium velocity
algorithms retain their Apache-2.0 and BSD-3-Clause notices in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
