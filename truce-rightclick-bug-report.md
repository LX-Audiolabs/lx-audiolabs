- [x] I searched existing issues and this is not a duplicate.
- [x] I am on a recent version and the bug still reproduces.

## What happened?

Right-click (`Mouse::Button::Right`) events never reach iced canvas widgets. This breaks right-click-to-reset in all knob and slider widgets (both `truce-iced` built-in widgets AND custom widgets via `canvas::Program`). Left-click drag works correctly.

## Steps to reproduce

1. Build any CLAP plugin using `truce-iced` 1.0.4
2. Load plugin in a DAW (Reaper / Bitwig)
3. Left-click-drag a knob → works
4. Right-click the same knob → nothing happens

## Expected behaviour

Right-click resets the parameter to its default value. Both `truce-iced`'s own `knob.rs` and `slider.rs` widgets have explicit right-click handlers — they're just never invoked because the event never arrives.

## Root Cause

In `truce-iced-1.0.4/src/editor.rs`, the `baseview::MouseEvent::ButtonPressed` and `ButtonReleased` handlers **only match `baseview::MouseButton::Left`**. `Right`, `Middle`, and `Other` are silently dropped.

```rust
// BEFORE (truce-iced 1.0.4, line 433): only Left mapped
baseview::MouseEvent::ButtonPressed {
    button: baseview::MouseButton::Left,  // ← restrictive pattern match
    ..
} => {
    runtime.pending_events.push(Event::Mouse(
        crate::iced::mouse::Event::ButtonPressed(
            crate::iced::mouse::Button::Left,  // ← hardcoded Left
        ),
    ));
}
// Same pattern at line 454 for ButtonReleased
```

## Fix

Use a generic `button` binding and map all buttons:

```rust
// AFTER (fix): all buttons mapped
baseview::MouseEvent::ButtonPressed { button, .. } => {
    let iced_button = match button {
        baseview::MouseButton::Left   => crate::iced::mouse::Button::Left,
        baseview::MouseButton::Right  => crate::iced::mouse::Button::Right,
        baseview::MouseButton::Middle => crate::iced::mouse::Button::Middle,
        _ => return baseview::EventStatus::Ignored,
    };
    runtime.pending_events.push(Event::Mouse(
        crate::iced::mouse::Event::ButtonPressed(iced_button),
    ));
}
// Same for ButtonReleased
```

We've verified this fix locally via `[patch.crates-io]` — right-click reset now works across all plugins.

## Plugin format

- [x] CLAP
- [ ] VST3
- [ ] VST2
- [ ] LV2
- [ ] AU v2
- [ ] AU v3
- [ ] AAX

## Workspace version

truce 1.0.4 / truce-iced 1.0.4 / truce-clap 1.0.4

## OS and rust version

Windows 11 / rustc 1.96.0 (ac68faa20 2026-05-25)
