//! `htl::serde` and `htl::serde_json`: a type derives serde through htl's re-export,
//! naming it with `#[serde(crate = "htl::serde")]` as a host without a `serde`
//! dependency of its own has to.
//!
//! This test crate does have `serde` as a dev-dependency (the C ABI tests use it), so it
//! cannot show the absence of one. What it shows is the part that matters: with
//! `crate = "htl::serde"`, every path serde's derive writes goes through `htl::serde`
//! rather than `::serde`, and those paths resolve.

#[derive(htl::serde::Serialize, htl::serde::Deserialize, Debug, PartialEq)]
#[serde(crate = "htl::serde")]
struct Call {
    #[serde(rename = "callId")]
    call_id: String,
    retries: u32,
}

#[test]
fn a_type_derives_through_htl_serde_and_round_trips_through_htl_serde_json() {
    let c = Call {
        call_id: "a1".into(),
        retries: 2,
    };
    let s = htl::serde_json::to_string(&c).unwrap();
    assert_eq!(s, r#"{"callId":"a1","retries":2}"#);
    let back: Call = htl::serde_json::from_str(&s).unwrap();
    assert_eq!(back, c);
}
