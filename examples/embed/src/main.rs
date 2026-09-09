//! Host binary embedding Teal scripts.
//!
//! - `include_tl!` / `include_tl_bytes!` type-check each `.tl` at `cargo build` time and
//!   embed generated Lua (source or stripped bytecode). No `.tl` is read at runtime.
//! - `#[derive(TealRecord)]` mirrors `Point` as a Teal record (plain table both ways),
//!   `Mode` as a Teal `enum` (a string both ways), `Label` as a `type` alias of `string`,
//!   and `Shape` — an enum carrying data — as a union of `where`-discriminated records
//!   (a table with a `kind` field).
//! - `#[host_module(records = [Point, Mode, Label, Shape])]` turns the plain `impl Host`
//!   into a `UserData` impl and writes `scripts/host.d.tl` with those nested inside the
//!   module record, so Teal sees the Rust API with its real signatures as `host.Point`,
//!   `host.Mode`, `host.Label`, `host.Shape` and narrows with `is host.Shape_Circle`.

use anyhow::Result;
use htl::{Htl, TealRecord, host_module, include_tl, include_tl_bytes};
use std::time::Instant;

#[derive(TealRecord, Debug, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A closed set: `host.Mode` is a Teal `enum`, so `host:pace("fst")` is a check error
/// and a `"fst"` that reaches the host at run time is refused by name.
#[derive(TealRecord, Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fast,
    Careful,
}

/// A newtype: `host.Label` is `type Label = string`, and crosses as the string.
#[derive(TealRecord, Debug, Clone)]
pub struct Label(pub String);

/// An enum with data: one record per variant (`host.Shape_Dot`, `host.Shape_Circle`,
/// `host.Shape_Rect`) discriminated on `kind`, and `host.Shape` their union.
#[derive(TealRecord, Debug, Clone, PartialEq)]
pub enum Shape {
    Dot,
    Circle(f64),
    Rect { w: f64, h: f64 },
}

pub struct Host {
    started: Instant,
}

#[host_module(name = "host", dts = "scripts/host.d.tl", records = [Point, Mode, Label, Shape])]
impl Host {
    /// Seconds since the Unix epoch.
    pub fn now(&self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Milliseconds since the host started.
    pub fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Record in, record out: `host.Point` crosses the boundary as a plain table.
    pub fn scale(&self, p: Point, k: f64) -> Point {
        Point {
            x: p.x * k,
            y: p.y * k,
        }
    }

    /// `&str` parameter: Teal passes a string, the wrapper hands the fn a `&String`.
    pub fn greet(name: &str) -> String {
        format!("hello from Rust, {name}")
    }

    /// `&[f64]` parameter: Teal passes `{number}`, the wrapper hands the fn a `&Vec<f64>`.
    pub fn sum(xs: &[f64]) -> f64 {
        xs.iter().sum()
    }

    /// `&Point` parameter: borrowed record.
    pub fn norm(&self, p: &Point) -> f64 {
        (p.x * p.x + p.y * p.y).sqrt()
    }

    /// `Result` return: `Err` becomes a Lua error.
    pub fn parse_int(s: &str) -> Result<i64, std::num::ParseIntError> {
        s.trim().parse()
    }

    /// Enum in, newtype out: Teal passes `"Fast"` / `"Careful"` and gets a string back.
    pub fn pace(&self, m: Mode) -> Label {
        Label(match m {
            Mode::Fast => "hurry".to_string(),
            Mode::Careful => "take your time".to_string(),
        })
    }

    /// Data enum in: Teal builds `{ kind = "Circle", value = 2 }` and the host matches.
    pub fn area(&self, s: Shape) -> f64 {
        match s {
            Shape::Dot => 0.0,
            Shape::Circle(r) => std::f64::consts::PI * r * r,
            Shape::Rect { w, h } => w * h,
        }
    }

    /// Data enum out: Teal narrows the result with `is host.Shape_Rect`.
    pub fn bounding(&self, w: f64, h: f64) -> Shape {
        if w == h && w == 0.0 {
            Shape::Dot
        } else if w == h {
            Shape::Circle(w / 2.0)
        } else {
            Shape::Rect { w, h }
        }
    }
}

/// Same idea with `errors = "return"`: `Result` comes back Lua-style as `value, err`
/// (`true, nil` / `nil, "message"` for unit) instead of raising.
pub struct Store {
    dir: std::path::PathBuf,
}

#[host_module(name = "store", dts = "scripts/store.d.tl", errors = "return")]
impl Store {
    pub fn write(&self, name: &str, text: &str) -> Result<(), std::io::Error> {
        if name.contains('/') {
            return Err(std::io::Error::other(format!("invalid name: {name}")));
        }
        std::fs::write(self.dir.join(name), text)
    }

    pub fn read(&self, name: &str) -> Result<String, std::io::Error> {
        std::fs::read_to_string(self.dir.join(name))
    }
}

// The `.d.tl` above is written when `#[host_module]` expands, which happens before
// `include_tl!` below is expanded (same file, source order).
const MAIN: &str = include_tl!("scripts/main.tl");
const UTIL: &[u8] = include_tl_bytes!("scripts/util.tl");

// The same program as one linked bundle: main + util, with `host` and `store` recorded
// as host-provided (they resolve only to `.d.tl`). Run with `--bundle`.
const BUNDLE: &[u8] = htl::include_bundle!("scripts/main.tl");

// `cargo build -p embed --features bad` -> Teal type error surfaces as a Rust compile error.
#[cfg(feature = "bad")]
const BAD: &str = include_tl!("scripts/bad.tl");

fn main() -> Result<()> {
    let h = Htl::new()?;
    Host {
        started: Instant::now(),
    }
    .htl_preload(&h)?;
    Store {
        dir: std::env::temp_dir(),
    }
    .htl_preload(&h)?;

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--bundle") {
        args.remove(0);
        // Everything but the host modules comes from the bundle; util is not preloaded.
        h.run_bundle(&htl::bundle::Bundle::decode(BUNDLE)?, &args)?;
        return Ok(());
    }
    h.preload_bytes("util", UTIL)?; // stripped: its frames read `?`, with no line
    // Source, named for the file it came from: its frames read `scripts/main.tl:<line>`.
    h.exec(MAIN, "@scripts/main.tl", &args)?;
    Ok(())
}
