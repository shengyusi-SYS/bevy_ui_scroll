# Third-party notices

Original code is licensed under MIT (see LICENSE). Adapted algorithms retain the notices below.

原创代码采用 MIT，见 LICENSE。改编算法保留以下来源与许可声明。

## Android OverScroller

`src/fling.rs` adapts spline fling constants, duration/distance formulas and lookup-table construction from Android `OverScroller.java`.

`src/fling.rs` 改编自 Android `OverScroller.java` 的惯性样条常量、时长与距离公式，以及查找表构造。

Copyright (C) 2010 The Android Open Source Project.
Licensed under the Apache License, Version 2.0. Full text: `licenses/Apache-2.0.txt`.
Source: <https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/widget/OverScroller.java>.
Changes: Rust implementation, Bevy ECS integration, two-axis animation and boundary handling. This crate does not include Android platform code outside the adapted algorithms.

## Chromium velocity tracking

`src/velocity.rs` derives from Chromium VelocityTracker's LSQ2_RESTRICTED strategy and uses a Rust least-squares solver.

`src/velocity.rs` 的来源是 Chromium VelocityTracker 的 LSQ2_RESTRICTED 策略，使用 Rust 最小二乘求解实现。

Copyright 2014 The Chromium Authors.
BSD-style license: `licenses/Chromium-BSD.txt` (upstream repository license includes Copyright 2015 The Chromium Authors).
Source: <https://chromium.googlesource.com/chromium/src/+/24d53e3e9a861c76d0e1626522c93e371f5d9223/ui/events/velocity_tracker/velocity_tracker.cc>.
Changes: bounded Rust sample storage, normal-equation fit and Bevy vector integration.
