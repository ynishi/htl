//! A macroquad window for an htl program.
//!
//! [`Mq`] is a [`#[host_module]`](htl::host_module) that re-exports macroquad's drawing
//! and input to Teal as `require("mq")`. [`run`] drives a Teal game table (with
//! `load`, `update(dt): boolean`, and `draw` methods) through the frame loop.
//! [`Hooks`] provides the two environment hooks a run without a person at the window
//! needs. The declaration file `dts/mq.d.tl` is written by the `#[host_module]` macro
//! at build time and shipped through `[package.metadata.htl] dts`. [`macroquad`] is
//! re-exported, so a project that draws on its own side reaches it through this crate
//! rather than naming a version of its own.

use htl::mlua::{Function, Table};
use htl::{Htl, TealRecord, host_module};
use std::cell::RefCell;
use std::rc::Rc;

/// The macroquad this window was opened with. A project's own host module — `fx` in what
/// `htl new --target window` writes — draws into the same frame as [`Mq`] does, and two
/// macroquads in one binary are two sets of process-global state; taking it from here is
/// what makes it the same one, and it costs the project no version of its own to keep in
/// step with this crate's.
pub use macroquad;

/// Channels 0.0..=1.0, as macroquad's.
#[derive(TealRecord, Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl From<Color> for macroquad::color::Color {
    fn from(c: Color) -> Self {
        macroquad::color::Color::new(c.r, c.g, c.b, c.a)
    }
}

impl From<macroquad::color::Color> for Color {
    fn from(c: macroquad::color::Color) -> Self {
        Color {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}

/// A 2D vector.
#[derive(TealRecord, Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl From<Vec2> for macroquad::math::Vec2 {
    fn from(v: Vec2) -> Self {
        macroquad::math::Vec2::new(v.x, v.y)
    }
}

impl From<macroquad::math::Vec2> for Vec2 {
    fn from(v: macroquad::math::Vec2) -> Self {
        Vec2 { x: v.x, y: v.y }
    }
}

/// The subset of macroquad's `KeyCode` a game reaches for. On the Teal side it is an
/// `enum`, so a string outside the set is an `htl check` error that lists the variants,
/// and one that reaches the host at run time is refused by name.
#[derive(TealRecord, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftAlt,
    RightAlt,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Key0,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl From<Key> for macroquad::input::KeyCode {
    fn from(k: Key) -> Self {
        match k {
            Key::Space => macroquad::input::KeyCode::Space,
            Key::Escape => macroquad::input::KeyCode::Escape,
            Key::Enter => macroquad::input::KeyCode::Enter,
            Key::Tab => macroquad::input::KeyCode::Tab,
            Key::Backspace => macroquad::input::KeyCode::Backspace,
            Key::Delete => macroquad::input::KeyCode::Delete,
            Key::Insert => macroquad::input::KeyCode::Insert,
            Key::Home => macroquad::input::KeyCode::Home,
            Key::End => macroquad::input::KeyCode::End,
            Key::PageUp => macroquad::input::KeyCode::PageUp,
            Key::PageDown => macroquad::input::KeyCode::PageDown,
            Key::Up => macroquad::input::KeyCode::Up,
            Key::Down => macroquad::input::KeyCode::Down,
            Key::Left => macroquad::input::KeyCode::Left,
            Key::Right => macroquad::input::KeyCode::Right,
            Key::LeftShift => macroquad::input::KeyCode::LeftShift,
            Key::RightShift => macroquad::input::KeyCode::RightShift,
            Key::LeftControl => macroquad::input::KeyCode::LeftControl,
            Key::RightControl => macroquad::input::KeyCode::RightControl,
            Key::LeftAlt => macroquad::input::KeyCode::LeftAlt,
            Key::RightAlt => macroquad::input::KeyCode::RightAlt,
            Key::A => macroquad::input::KeyCode::A,
            Key::B => macroquad::input::KeyCode::B,
            Key::C => macroquad::input::KeyCode::C,
            Key::D => macroquad::input::KeyCode::D,
            Key::E => macroquad::input::KeyCode::E,
            Key::F => macroquad::input::KeyCode::F,
            Key::G => macroquad::input::KeyCode::G,
            Key::H => macroquad::input::KeyCode::H,
            Key::I => macroquad::input::KeyCode::I,
            Key::J => macroquad::input::KeyCode::J,
            Key::K => macroquad::input::KeyCode::K,
            Key::L => macroquad::input::KeyCode::L,
            Key::M => macroquad::input::KeyCode::M,
            Key::N => macroquad::input::KeyCode::N,
            Key::O => macroquad::input::KeyCode::O,
            Key::P => macroquad::input::KeyCode::P,
            Key::Q => macroquad::input::KeyCode::Q,
            Key::R => macroquad::input::KeyCode::R,
            Key::S => macroquad::input::KeyCode::S,
            Key::T => macroquad::input::KeyCode::T,
            Key::U => macroquad::input::KeyCode::U,
            Key::V => macroquad::input::KeyCode::V,
            Key::W => macroquad::input::KeyCode::W,
            Key::X => macroquad::input::KeyCode::X,
            Key::Y => macroquad::input::KeyCode::Y,
            Key::Z => macroquad::input::KeyCode::Z,
            Key::Key0 => macroquad::input::KeyCode::Key0,
            Key::Key1 => macroquad::input::KeyCode::Key1,
            Key::Key2 => macroquad::input::KeyCode::Key2,
            Key::Key3 => macroquad::input::KeyCode::Key3,
            Key::Key4 => macroquad::input::KeyCode::Key4,
            Key::Key5 => macroquad::input::KeyCode::Key5,
            Key::Key6 => macroquad::input::KeyCode::Key6,
            Key::Key7 => macroquad::input::KeyCode::Key7,
            Key::Key8 => macroquad::input::KeyCode::Key8,
            Key::Key9 => macroquad::input::KeyCode::Key9,
            Key::F1 => macroquad::input::KeyCode::F1,
            Key::F2 => macroquad::input::KeyCode::F2,
            Key::F3 => macroquad::input::KeyCode::F3,
            Key::F4 => macroquad::input::KeyCode::F4,
            Key::F5 => macroquad::input::KeyCode::F5,
            Key::F6 => macroquad::input::KeyCode::F6,
            Key::F7 => macroquad::input::KeyCode::F7,
            Key::F8 => macroquad::input::KeyCode::F8,
            Key::F9 => macroquad::input::KeyCode::F9,
            Key::F10 => macroquad::input::KeyCode::F10,
            Key::F11 => macroquad::input::KeyCode::F11,
            Key::F12 => macroquad::input::KeyCode::F12,
        }
    }
}

#[derive(TealRecord, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl From<MouseButton> for macroquad::input::MouseButton {
    fn from(b: MouseButton) -> Self {
        match b {
            MouseButton::Left => macroquad::input::MouseButton::Left,
            MouseButton::Middle => macroquad::input::MouseButton::Middle,
            MouseButton::Right => macroquad::input::MouseButton::Right,
        }
    }
}

/// Stateless, because macroquad keeps its state process-global; every method calls
/// macroquad and panics when no window is open, which is why a Teal test never calls
/// them — `htl test` sees only the declaration.
pub struct Mq;

#[host_module(name = "mq", dts = "dts/mq.d.tl", records = [Color, Vec2, Key, MouseButton])]
impl Mq {
    /// Returns the current screen width.
    pub fn screen_width(&self) -> f32 {
        macroquad::window::screen_width()
    }

    /// Returns the current screen height.
    pub fn screen_height(&self) -> f32 {
        macroquad::window::screen_height()
    }

    /// Returns the time elapsed since the last frame.
    pub fn frame_time(&self) -> f32 {
        macroquad::time::get_frame_time()
    }

    /// Returns the time since application start.
    pub fn time(&self) -> f64 {
        macroquad::time::get_time()
    }

    /// Returns the current FPS.
    pub fn fps(&self) -> i32 {
        macroquad::time::get_fps()
    }

    /// Clears the background with the given color.
    pub fn clear_background(&self, color: Color) {
        macroquad::window::clear_background(color.into());
    }

    /// Draws a filled rectangle.
    pub fn draw_rectangle(&self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        macroquad::shapes::draw_rectangle(x, y, w, h, color.into());
    }

    /// Draws a rectangle outline.
    pub fn draw_rectangle_lines(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        thickness: f32,
        color: Color,
    ) {
        macroquad::shapes::draw_rectangle_lines(x, y, w, h, thickness, color.into());
    }

    /// Draws a filled circle.
    pub fn draw_circle(&self, x: f32, y: f32, r: f32, color: Color) {
        macroquad::shapes::draw_circle(x, y, r, color.into());
    }

    /// Draws a circle outline.
    pub fn draw_circle_lines(&self, x: f32, y: f32, r: f32, thickness: f32, color: Color) {
        macroquad::shapes::draw_circle_lines(x, y, r, thickness, color.into());
    }

    /// Draws a line.
    pub fn draw_line(&self, x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: Color) {
        macroquad::shapes::draw_line(x1, y1, x2, y2, thickness, color.into());
    }

    /// Draws text.
    pub fn draw_text(&self, text: &str, x: f32, y: f32, font_size: f32, color: Color) {
        macroquad::text::draw_text(text, x, y, font_size, color.into());
    }

    /// Returns whether the given key is currently held down.
    pub fn is_key_down(&self, key: Key) -> bool {
        macroquad::input::is_key_down(key.into())
    }

    /// Returns whether the given key was pressed this frame.
    pub fn is_key_pressed(&self, key: Key) -> bool {
        macroquad::input::is_key_pressed(key.into())
    }

    /// Returns whether the given key was released this frame.
    pub fn is_key_released(&self, key: Key) -> bool {
        macroquad::input::is_key_released(key.into())
    }

    /// Returns the current mouse position.
    pub fn mouse_position(&self) -> Vec2 {
        let (x, y) = macroquad::input::mouse_position();
        Vec2 { x, y }
    }

    /// Returns whether the given mouse button is currently held down.
    pub fn is_mouse_button_down(&self, button: MouseButton) -> bool {
        macroquad::input::is_mouse_button_down(button.into())
    }

    /// Returns whether the given mouse button was pressed this frame.
    pub fn is_mouse_button_pressed(&self, button: MouseButton) -> bool {
        macroquad::input::is_mouse_button_pressed(button.into())
    }
}

/// What a run without a person at the window needs: stop after `frames` frames, and
/// write the last frame drawn to `shot` as a PNG. Read from `HTL_MQ_FRAMES` and
/// `HTL_MQ_SHOT` by [`Hooks::from_env`], which [`run`] does; [`run_with`] takes them
/// explicitly, and [`Hooks::NONE`] turns both off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    pub frames: Option<u64>,
    pub shot: Option<String>,
}

impl Hooks {
    pub const NONE: Hooks = Hooks {
        frames: None,
        shot: None,
    };

    /// `HTL_MQ_FRAMES` / `HTL_MQ_SHOT` from the environment, through [`Hooks::parse`].
    pub fn from_env() -> anyhow::Result<Hooks> {
        Self::parse(
            std::env::var("HTL_MQ_FRAMES").ok().as_deref(),
            std::env::var("HTL_MQ_SHOT").ok().as_deref(),
        )
    }

    /// `frames` must be a `u64` of 1 or more; `shot` is taken as written. `None` or an
    /// empty string is "unset" for either.
    pub fn parse(frames: Option<&str>, shot: Option<&str>) -> anyhow::Result<Hooks> {
        let frames = match frames {
            None | Some("") => None,
            Some(s) => {
                let n: u64 = s.parse().map_err(|_| {
                    anyhow::anyhow!("HTL_MQ_FRAMES: expected a frame count of 1 or more, got `{s}`")
                })?;
                if n == 0 {
                    anyhow::bail!("HTL_MQ_FRAMES: expected a frame count of 1 or more, got `{s}`");
                }
                Some(n)
            }
        };
        let shot = match shot {
            None | Some("") => None,
            Some(s) => Some(s.to_string()),
        };
        Ok(Hooks { frames, shot })
    }
}

/// A window with a title and a size; everything else is macroquad's default.
pub fn conf(title: &str, width: i32, height: i32) -> macroquad::conf::Conf {
    macroquad::conf::Conf {
        miniquad_conf: macroquad::miniquad::conf::Conf {
            window_title: title.to_string(),
            window_width: width,
            window_height: height,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The functions a game table provides. `load` may be absent; the other two may not.
struct Game {
    load: Option<Function>,
    update: Function,
    draw: Function,
}

fn game_of(table: &Table) -> anyhow::Result<Game> {
    let load = table.get::<Option<Function>>("load")?;
    let update = table
        .get::<Option<Function>>("update")?
        .ok_or_else(|| anyhow::anyhow!("the game table has no `update` function"))?;
    let draw = table
        .get::<Option<Function>>("draw")?
        .ok_or_else(|| anyhow::anyhow!("the game table has no `draw` function"))?;
    Ok(Game { load, update, draw })
}

/// Open the window described by `conf` and drive `game` until `update` returns false,
/// the window is closed, or the `HTL_MQ_FRAMES` hook is reached. `h` is the state the
/// table lives in; it is kept alive for the whole run. Hooks come from the
/// environment; [`run_with`] is the same with them given.
pub fn run(h: Htl, game: Table, conf: macroquad::conf::Conf) -> anyhow::Result<()> {
    run_with(h, game, conf, Hooks::from_env()?)
}

/// [`run`] with the hooks given rather than read from the environment.
pub fn run_with(
    h: Htl,
    game: Table,
    conf: macroquad::conf::Conf,
    hooks: Hooks,
) -> anyhow::Result<()> {
    let game = game_of(&game)?;
    let failed: Rc<RefCell<Option<anyhow::Error>>> = Rc::new(RefCell::new(None));
    macroquad::Window::from_config(conf, {
        let failed = Rc::clone(&failed);
        async move {
            let _h = h;
            if let Err(e) = frames(game, hooks).await {
                *failed.borrow_mut() = Some(e);
            }
        }
    });
    match failed.borrow_mut().take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// One run of the game loop: `load` once, then `update(dt)` and `draw()` each frame
/// around `next_frame().await`, until `update` returns false or the frame hook is
/// reached; then the screenshot hook, if any.
async fn frames(game: Game, hooks: Hooks) -> anyhow::Result<()> {
    if let Some(f) = game.load {
        f.call::<()>(())?;
    }
    let mut n: u64 = 0;
    loop {
        let dt = macroquad::time::get_frame_time();
        let go: bool = game.update.call::<bool>(dt)?;
        // the game says it is done; the PNG, if asked for, holds the previous frame
        if !go {
            break;
        }
        game.draw.call::<()>(())?;
        n += 1;
        if hooks.frames == Some(n) {
            break;
        }
        macroquad::window::next_frame().await;
    }
    if let Some(path) = &hooks.shot {
        macroquad::texture::get_screen_data().export_png(path);
    }
    Ok(())
}
