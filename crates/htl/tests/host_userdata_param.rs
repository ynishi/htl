//! A `#[host_module]` method taking another host type: `UserDataRef<T>` on the Rust side,
//! declared as `T` on the Teal side — the name a method returning it already used.

use htl::mlua::UserDataRef;
use htl::teal::HostModule as _;
use htl::{Htl, host_module};

pub struct Session {
    id: u32,
}

#[host_module(name = "Session")]
impl Session {
    pub fn id(&self) -> u32 {
        self.id
    }
}

pub struct Knl;

#[host_module(name = "knl", uses = [Session])]
impl Knl {
    pub fn open(&self, id: u32) -> Session {
        Session { id }
    }
    pub fn resume(&self, parent: UserDataRef<Session>) -> u32 {
        parent.id
    }
}

#[test]
fn a_userdata_parameter_is_declared_as_the_host_type() {
    assert!(
        Knl::DECL.contains("resume: function(self: knl, parent: Session): integer"),
        "{}",
        Knl::DECL
    );
    assert!(Knl::DECL.starts_with("local type Session = require(\"Session\")\n"));
}

#[test]
fn a_session_returned_by_open_goes_back_in_through_resume() {
    let h = Htl::new().unwrap();
    Knl.htl_preload(&h).unwrap();
    let n: u32 = h
        .lua()
        .load("local knl = require('knl'); local s = knl:open(7); return knl:resume(s)")
        .eval()
        .unwrap();
    assert_eq!(n, 7);
}

#[test]
fn a_table_where_a_session_is_expected_is_refused() {
    let h = Htl::new().unwrap();
    Knl.htl_preload(&h).unwrap();
    let err = h
        .lua()
        .load("local knl = require('knl'); return knl:resume({})")
        .eval::<u32>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("resume"), "{err}");
}
