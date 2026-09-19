//! Client performance benchmark: ten fresh Chrome sessions, CPU throttling, no pass thresholds.
//! It lives outside the gating `browser` target so it never competes with the contracts:
//! AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser_performance -- --ignored

#[allow(dead_code)]
#[path = "browser/harness.rs"]
mod harness;

use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use harness::{Fixture, browser_client, emulate, finish, screenshot};

#[tokio::test]
#[ignore = "requires a local Chrome WebDriver"]
async fn reader_client_performance_metrics() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = Fixture::with_pwa(false)?;
    let mut measurements = Vec::new();
    for (environment, width, mobile, rate) in
        [("desktop", 1280, false, 1), ("mobile-4x", 390, true, 4)]
    {
        for run in 0..5 {
            let client = browser_client().await?;
            let measurement: Result<Value> = async {
                        emulate(&client, "Network.enable", json!({})).await?;
                        emulate(&client, "Network.setCacheDisabled", json!({"cacheDisabled": true})).await?;
                        emulate(&client, "Network.setBlockedURLs", json!({"urls": ["*.png", "*.jpg", "*.jpeg", "*.webp", "*.gif", "*.avif"]})).await?;
                        emulate(&client, "Emulation.setDeviceMetricsOverride", json!({"width": width, "height": 844, "deviceScaleFactor": 1, "mobile": mobile})).await?;
                        emulate(&client, "Emulation.setCPUThrottlingRate", json!({"rate": rate})).await?;
                        emulate(&client, "Page.addScriptToEvaluateOnNewDocument", json!({"source": "window.__aggrPerfErrors=[];addEventListener('error',e=>window.__aggrPerfErrors.push(e.message));addEventListener('unhandledrejection',e=>window.__aggrPerfErrors.push(String(e.reason)));requestAnimationFrame(function ready(){if(typeof window.swup?.navigate==='function'&&document.querySelector('.search-command'))window.__aggrPerfReadyAt=performance.now();else requestAnimationFrame(ready)})"})).await?;
                        client.goto(&fixture.base).await?;
                        let value = client.execute_async(r#"
                          const base=arguments[0],done=arguments[arguments.length-1];
                          (async()=>{
                            const until=async check=>{const started=performance.now();while(!check()){if(performance.now()-started>15000)throw Error('timed out waiting for performance scenario');await new Promise(requestAnimationFrame)}};
                            await until(()=>window.__aggrPerfReadyAt&&performance.getEntriesByName('first-contentful-paint').length);
                            const initialReadyMs=window.__aggrPerfReadyAt,fcpMs=performance.getEntriesByName('first-contentful-paint')[0].startTime;
                            let started=performance.now();
                            await window.swup.navigate(new URL('preferences/',base).href);
                            await until(()=>!window.swup.navigating&&document.querySelector('#theme-mode'));
                            const preferencesNavigationMs=performance.now()-started;
                            started=performance.now();
                            await window.swup.navigate(base);
                            await until(()=>!window.swup.navigating&&document.querySelector('.search-command'));
                            const feedNavigationMs=performance.now()-started;
                            const input=document.querySelector('#q');
                            started=performance.now();
                            input.value='category:engineering "Reading comfortably"';
                            input.setSelectionRange(input.value.length,input.value.length);
                            input.dispatchEvent(new Event('input',{bubbles:true}));
                            await until(()=>document.querySelector('.search-results .row'));
                            const firstSearchMs=performance.now()-started;
                            done({initialReadyMs,fcpMs,preferencesNavigationMs,feedNavigationMs,firstSearchMs,results:document.querySelectorAll('.search-results .row').length});
                          })().catch(error=>done({error:error.stack||String(error),ready:window.__aggrPerfReadyAt,swup:!!window.swup,search:!!document.querySelector('.search-command'),paint:performance.getEntriesByType('paint').map(e=>({name:e.name,start:e.startTime})),clientErrors:window.__aggrPerfErrors,url:location.href}));
                        "#, vec![json!(fixture.base)]).await?;
                        anyhow::ensure!(value.get("error").is_none(), "performance scenario ({environment}/{run}): {value}");
                        Ok(value)

            }.await;
            if measurement.is_err() {
                let _ = screenshot(&client, "client-performance-failure").await;
            }
            let mut measurement = finish(client, measurement).await?;
            measurement["environment"] = json!(environment);
            measurement["run"] = json!(run);
            measurements.push(measurement);
        }
    }
    let mut medians = Vec::new();
    for environment in ["desktop", "mobile-4x"] {
        let mut median = json!({"environment": environment});
        for metric in [
            "initialReadyMs",
            "fcpMs",
            "preferencesNavigationMs",
            "feedNavigationMs",
            "firstSearchMs",
        ] {
            let mut values: Vec<f64> = measurements
                .iter()
                .filter(|sample| sample["environment"] == environment)
                .filter_map(|sample| sample[metric].as_f64())
                .collect();
            values.sort_by(f64::total_cmp);
            median[metric] = json!(values[values.len() / 2]);
        }
        eprintln!("client performance median: {median}");
        medians.push(median);
    }
    let report = json!({"units": "milliseconds", "archiveItems": 45, "runsPerEnvironment": 5,
        "scope": "Current production client in fresh browser sessions, HTTP cache disabled and image requests blocked. Mobile emulates a 390px viewport with 4x CPU throttling. Local network, no pass thresholds or historical comparison.",
        "medians": medians, "measurements": measurements});
    let artifacts = Path::new("target/browser-artifacts");
    std::fs::create_dir_all(artifacts)?;
    std::fs::write(
        artifacts.join("client-performance.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
