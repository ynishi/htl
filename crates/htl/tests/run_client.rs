//! The client `htl rs` writes for an exported module (`[rust] exports`), compiled against
//! `htl::run` and called on a runner.
//!
//! The file is `htl-core/tests/fixtures/client/teal_types.rs`, the golden that `htl rs` and
//! `teal_types!()` are held to by the tests of `htl-cli` and `htl-macros`, brought in whole
//! by `include!`: what is compiled here is what is written. Its Teal is the `src/` beside it.
#![cfg(feature = "async")]

mod common;

use std::time::Duration;

include!("../../htl-core/tests/fixtures/client/teal_types.rs");

const TYPES_TL: &str = include_str!("../../htl-core/tests/fixtures/client/src/shop/types.tl");
const APP_TL: &str = include_str!("../../htl-core/tests/fixtures/client/src/shop/app.tl");

fn assert_send_sync<T: Send + Sync>() {}

#[tokio::test]
async fn the_generated_client_calls_the_teal_module() {
    assert_send_sync::<shop::app::App>();
    let root = common::tempdir("htl-run-client", "shop");
    std::fs::create_dir_all(root.join("shop")).unwrap();
    std::fs::write(root.join("shop/types.tl"), TYPES_TL).unwrap();
    std::fs::write(root.join("shop/app.tl"), APP_TL).unwrap();

    let (runner, driver) = htl::run::start(htl::run::RunnerConfig {
        search_paths: vec![root.to_path_buf()],
        deadline: Duration::from_secs(30),
        queue: 8,
    })
    .await
    .unwrap();
    let app = shop::app::App::load(&runner, &[]).await.unwrap();

    // A task of the host's runtime holds the client, and two results come back as a tuple.
    let task = {
        let app = app.clone();
        tokio::spawn(async move { app.split("abc".into()).await })
    };
    assert_eq!(task.await.unwrap().unwrap(), ("abc".to_string(), 3));

    // A record comes back as the struct the file generates, and `string | nil` takes `Some`
    // and `None`.
    let greeted = app.greet("Ada".into(), Some("Dr. ".into())).await.unwrap();
    assert_eq!(
        greeted,
        shop::types::Output {
            ok: true,
            text: "Dr. Ada".into()
        }
    );
    assert_eq!(
        app.greet("Ada".into(), None).await.unwrap().text,
        "Ada".to_string()
    );

    app.instance().clone().close().await.unwrap();
    driver.shutdown().await.unwrap();
}
