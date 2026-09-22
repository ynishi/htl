//! What a window crate can prove without a window: the declaration it ships, the
//! conversions to and from macroquad's own types, and the refusals at the boundary — a
//! key or a mouse button outside the set is named and rejected before the method body
//! reaches macroquad, and a game table missing `update` or `draw` before any window opens.

use htl::Htl;
use htl::mlua::Table;
use htl_mq::{Color, Hooks, Key, MouseButton, Mq, Vec2, conf, run_with};

#[test]
fn a_key_outside_the_set_is_refused_by_name_before_macroquad_is_touched() -> anyhow::Result<()> {
    let h = Htl::new()?;
    Mq.htl_preload(&h)?;
    let err = h
        .lua()
        .load("return require('mq'):is_key_pressed('spce')")
        .exec()
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Key"), "{msg}");
    assert!(msg.contains("\"Space\""), "{msg}");
    assert!(msg.contains("spce"), "{msg}");
    Ok(())
}

#[test]
fn a_mouse_button_outside_the_set_is_refused_by_name() -> anyhow::Result<()> {
    let h = Htl::new()?;
    Mq.htl_preload(&h)?;
    let err = h
        .lua()
        .load("return require('mq'):is_mouse_button_down('Center')")
        .exec()
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("MouseButton"), "{msg}");
    assert!(msg.contains("\"Left\""), "{msg}");
    assert!(msg.contains("Center"), "{msg}");
    Ok(())
}

#[test]
fn color_and_vec2_cross_into_macroquad_and_back() {
    let c = Color {
        r: 0.1,
        g: 0.2,
        b: 0.3,
        a: 0.4,
    };
    let m: macroquad::color::Color = c.into();
    assert_eq!((m.r, m.g, m.b, m.a), (0.1, 0.2, 0.3, 0.4));
    assert_eq!(Color::from(m), c);

    let v = Vec2 { x: 3.0, y: -4.5 };
    let m: macroquad::math::Vec2 = v.into();
    assert_eq!((m.x, m.y), (3.0, -4.5));
    assert_eq!(Vec2::from(m), v);
}

#[test]
fn a_key_maps_to_the_keycode_of_the_same_name() {
    use macroquad::input::KeyCode;
    assert_eq!(KeyCode::from(Key::Space), KeyCode::Space);
    assert_eq!(KeyCode::from(Key::Escape), KeyCode::Escape);
    assert_eq!(KeyCode::from(Key::Enter), KeyCode::Enter);
    assert_eq!(KeyCode::from(Key::Up), KeyCode::Up);
    assert_eq!(KeyCode::from(Key::A), KeyCode::A);
    assert_eq!(KeyCode::from(Key::Z), KeyCode::Z);
    assert_eq!(KeyCode::from(Key::Key0), KeyCode::Key0);
    assert_eq!(KeyCode::from(Key::Key9), KeyCode::Key9);
    assert_eq!(KeyCode::from(Key::F1), KeyCode::F1);
    assert_eq!(KeyCode::from(Key::F12), KeyCode::F12);
    assert_eq!(KeyCode::from(Key::LeftShift), KeyCode::LeftShift);
    assert_eq!(
        macroquad::input::MouseButton::from(MouseButton::Right),
        macroquad::input::MouseButton::Right
    );
}

#[test]
fn hooks_parse_reads_a_count_and_a_path_and_treats_empty_as_unset() -> anyhow::Result<()> {
    assert_eq!(Hooks::parse(None, None)?, Hooks::NONE);
    assert_eq!(Hooks::parse(Some(""), Some(""))?, Hooks::NONE);
    assert_eq!(
        Hooks::parse(Some("60"), Some("out.png"))?,
        Hooks {
            frames: Some(60),
            shot: Some("out.png".to_string())
        }
    );
    assert_eq!(
        Hooks::parse(None, Some("shot.png"))?,
        Hooks {
            frames: None,
            shot: Some("shot.png".to_string())
        }
    );
    Ok(())
}

#[test]
fn hooks_parse_refuses_a_count_that_is_not_one_or_more() {
    for s in ["0", "sixty", "-1"] {
        let msg = Hooks::parse(Some(s), None).unwrap_err().to_string();
        assert!(msg.contains("HTL_MQ_FRAMES"), "{msg}");
        assert!(msg.contains(s), "{msg}");
    }
}

#[test]
fn a_game_table_without_update_is_refused_before_a_window_opens() -> anyhow::Result<()> {
    let h = Htl::new()?;
    let t: Table = h.lua().load("return { draw = function() end }").eval()?;
    let msg = run_with(h, t, conf("headless", 64, 64), Hooks::NONE)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("update"), "{msg}");
    Ok(())
}

#[test]
fn a_game_table_without_draw_is_refused_before_a_window_opens() -> anyhow::Result<()> {
    let h = Htl::new()?;
    let t: Table = h
        .lua()
        .load("return { update = function() return false end }")
        .eval()?;
    let msg = run_with(h, t, conf("headless", 64, 64), Hooks::NONE)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("draw"), "{msg}");
    Ok(())
}

#[test]
fn the_declaration_on_disk_is_the_one_the_manifest_ships() {
    let dts = include_str!("../dts/mq.d.tl");
    for needle in [
        "local record mq",
        "enum Key",
        "\"Space\"",
        "\"Escape\"",
        "record Color",
        "record Vec2",
        "enum MouseButton",
        "is_key_pressed: function(self: mq, key: Key): boolean",
        "draw_text: function(self: mq, text: string, x: number, y: number, font_size: number, color: Color)",
        "mouse_position: function(self: mq): Vec2",
        "fps: function(self: mq): integer",
    ] {
        assert!(dts.contains(needle), "dts/mq.d.tl lacks {needle}");
    }
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("dts = [\"dts/mq.d.tl\"]"),
        "Cargo.toml lacks dts = [\"dts/mq.d.tl\"]"
    );
}
