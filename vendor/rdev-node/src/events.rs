use super::enums::*;
use napi_derive::napi;

/// Represents a key press event
#[napi(object)]
pub struct KeyPressEvent {
  pub key: KeyCode,
}

/// Represents a key release event
#[napi(object)]
pub struct KeyReleaseEvent {
  pub key: KeyCode,
}

/// Represents a mouse move event
#[napi(object)]
pub struct MouseMoveEvent {
  pub x: f64,
  pub y: f64,
}

/// Represents a button press event
#[napi(object)]
pub struct ButtonPressEvent {
  pub button: ButtonType,
}

/// Represents a button release event
#[napi(object)]
pub struct ButtonReleaseEvent {
  pub button: ButtonType,
}

/// Represents a wheel event
#[napi(object)]
pub struct WheelEvent {
  pub delta_x: i64,
  pub delta_y: i64,
}

/// Represents an input event
#[napi(object)]
pub struct InputEvent {
  /// The type of event
  pub event_type: EventTypeValue,
  /// Key press data (only present for KeyPress events)
  pub key_press: Option<KeyPressEvent>,
  /// Key release data (only present for KeyRelease events)
  pub key_release: Option<KeyReleaseEvent>,
  /// Mouse move data (only present for MouseMove events)
  pub mouse_move: Option<MouseMoveEvent>,
  /// Button press data (only present for ButtonPress events)
  pub button_press: Option<ButtonPressEvent>,
  /// Button release data (only present for ButtonRelease events)
  pub button_release: Option<ButtonReleaseEvent>,
  /// Wheel data (only present for Wheel events)
  pub wheel: Option<WheelEvent>,
  /// Optional name of window (platform-dependent)
  pub name: Option<String>,
  /// Timestamp of the event
  pub time: f64,
}

/// Display size information
#[napi(object)]
pub struct DisplaySize {
  pub width: f64,
  pub height: f64,
}
