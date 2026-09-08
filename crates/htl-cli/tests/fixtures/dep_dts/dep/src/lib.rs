//! The host side of the `dep` module. In a real crate this is
//! `#[host_module(name = "dep", dts = "dts/dep.d.tl")] impl Dep { .. }`, and the macro
//! keeps `dts/dep.d.tl` current; the fixture holds the written declaration and no
//! dependency, so that resolving this graph needs no network.

pub struct Dep;

impl Dep {
    pub fn greet(&self, name: &str) -> String {
        format!("hello, {name}")
    }
}
