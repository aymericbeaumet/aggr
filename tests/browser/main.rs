//! Browser contracts against a generated, pinned archive. Run with a local WebDriver:
//! AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored
//!
//! Every test builds its own fixture site and opens its own Chrome session, so the suite can run
//! with several test threads; `harness` holds everything they share.

mod harness;

mod article;
mod feed;
mod media;
mod mobile;
mod navigation;
mod no_js;
mod offline;
mod preferences;
mod responsive;
mod search;
