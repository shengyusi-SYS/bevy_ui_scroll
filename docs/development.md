# Development

[简体中文](development.zh-CN.md) · [Usage](../README.md)

## Set up

Install Rust 1.95 or newer with Cargo and rustfmt. The crate uses official Bevy
0.19.1. Run commands from this repository's root.

On Ubuntu, install the desktop build libraries:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxinerama-dev
```

On Windows, install the Visual Studio C++ build tools and Windows SDK. On macOS,
install the Xcode command-line tools with `xcode-select --install`. Running the
example uses a desktop session and a graphics driver supported by Bevy.

## Build and check

```sh
cargo fmt --all -- --check
cargo check --all-targets
cargo test --lib
cargo doc --no-deps
cargo run --example basic
```

Tests exercise geometry, ECS systems and layout in memory. Use the windowed
example to check input and appearance. Generated API docs are in
`target/doc/bevy_ui_scroll/index.html`. CI runs formatting, compilation, unit tests and
API documentation on Linux with stable Rust.

## Source map

| File | Responsibility |
| --- | --- |
| `src/input.rs`, `src/event/` | Pick a viewport, route wheel/touch input and hand gestures between nested views |
| `src/navigation.rs`, `src/motion.rs` | Offset commands, cancellation and shared spring math |
| `src/fling.rs`, `src/velocity.rs`, `src/overscroll.rs` | Touch velocity, inertia and boundary return |
| `src/snap.rs`, `src/snap/` | Snap targets and selected/settled state |
| `src/gesture_capture.rs` | Touch ownership for controls |
| `src/pull_to_refresh.rs` | Refresh gesture, controlled state and default indicator |
| `examples/basic.rs` | Runnable interaction demo |

Input resolves a viewport and stops its previous motion. Update advances the
active motion; PostUpdate resolves snap requests and observes the result after
`UiSystems::Layout`. `RequestRedraw` keeps active motion progressing in reactive
windows. Refresh work is started and completed by the caller.

When changing input or motion, exercise wheel and pixel input, touch release and
cancellation, nested boundaries, scrollbar takeover and snap after resize. For
refresh changes, check a short pull, threshold crossing, retreat, release and
completion. The example's status text helps compare requested and rendered offsets.

## Make a change

Start from the affected public component or event and follow it into the system
that consumes it. Add a focused regression test for changed behavior. Keep the
library's data expressed as Bevy entities, components and geometry; application
content enters through the public events and binding points.

Update the English and Chinese README together when a public API or setup step
changes. Keep development instructions in this guide and API details beside the
public types. Run the checks above before opening a pull request, and describe
the user-visible result and how you checked it.

## Release

Update the package version and the Git tag in both README installation examples.
Run the checks, commit the release, then tag that commit as `vX.Y.Z` and push the
commit and tag. Create a GitHub release with concise English and Chinese notes.
Keep the bundled license files with the source.
